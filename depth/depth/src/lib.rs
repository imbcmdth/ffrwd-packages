//! Monocular depth: every frame leaves as a gray map of how far away it is,
//! near bright and far dark, the size of the frame it was made from.
//!
//! The graph is Depth Anything V2 (small), run through `wasi:nn`. The module
//! never opens a file - the host binds the graph to a name with
//! `-nn depth=<path>` and this module asks for that name and nothing else.
//!
//! Frames arrive as rgba, or as yuv420p converted here by ffrwd-frame in the
//! range and matrix the stream declares. Each one is letterboxed into the
//! square the graph takes, normalized the way the model was trained, run,
//! min-max normalized over the frame's own range, and sampled back out to the
//! frame's own size as one byte a pixel.

// `generate_all`: the world's interfaces are wasi:nn's, a package of its own,
// and without it bindgen expects them to have been generated somewhere else.
wit_bindgen::generate!({
    path: "wit-world",
    world: "ffrwd:depth/depth",
    generate_all,
});

use ffrwd_frame::yuv::{self, Colour, Yuv420p};
use ffrwd_frame::Rgba;
use ffrwd_node::{Bound, Init, Input, NoParams, Node, Out, Output, Result, Shape, Tick};
use wasi::nn::graph::{load_by_name, Graph};
use wasi::nn::inference::GraphExecutionContext;
use wasi::nn::tensor::{Tensor, TensorType};

/// The name the host binds the graph to. `-nn depth=<path>`.
const MODEL: &str = "depth";

/// The square the graph is run at. Depth Anything works in patches of 14, and
/// 518 is 37 of them - the size its own preprocessing uses.
const SIDE: usize = 518;

/// The graph's own names for the tensors it takes and returns.
const INPUT_NAME: &str = "pixel_values";
const OUTPUT_NAME: &str = "predicted_depth";

/// What the model was trained on: each channel rescaled to 0..1, then
/// standardized by these.
const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const STD: [f32; 3] = [0.229, 0.224, 0.225];

#[derive(Clone, Copy, Debug, PartialEq)]
enum Arrives {
    Rgba,
    Yuv420p(Colour),
}

/// Where the frame sits inside the square the graph is run at, once it has
/// been scaled to fit with its shape kept.
#[derive(Clone, Copy)]
struct Letterbox {
    /// Square pixels per frame pixel.
    scale: f32,
    /// Where the scaled frame starts inside the square.
    offset_x: f32,
    offset_y: f32,
    /// How much of the square the scaled frame covers.
    width: usize,
    height: usize,
}

impl Letterbox {
    fn new(width: usize, height: usize) -> Letterbox {
        let scale = (SIDE as f32 / width as f32).min(SIDE as f32 / height as f32);
        let scaled_w = ((width as f32 * scale).round() as usize).clamp(1, SIDE);
        let scaled_h = ((height as f32 * scale).round() as usize).clamp(1, SIDE);
        Letterbox {
            scale,
            offset_x: ((SIDE - scaled_w) / 2) as f32,
            offset_y: ((SIDE - scaled_h) / 2) as f32,
            width: scaled_w,
            height: scaled_h,
        }
    }

    /// Where a frame pixel lands in the square, in square coordinates.
    fn to_square(self, x: usize, y: usize) -> (f32, f32) {
        (
            self.offset_x + (x as f32 + 0.5) * self.scale - 0.5,
            self.offset_y + (y as f32 + 0.5) * self.scale - 0.5,
        )
    }
}

struct Model {
    letterbox: Letterbox,
    /// Held for the life of the instance: building it once is what keeps a
    /// provider's kernels from being chosen again per frame.
    context: GraphExecutionContext,
    /// Kept alive because the context is only valid while its graph is.
    _graph: Graph,
}

struct Depth {
    v: u32,
    width: usize,
    height: usize,
    arrives: Arrives,
    rgba: Vec<u8>,
    model: Model,
}

/// The spec's spelling of an error code, so a message says what actually
/// went wrong rather than how this module happens to format things.
fn failed(what: &str, error: &wasi::nn::errors::Error) -> String {
    use wasi::nn::errors::ErrorCode;
    let code = match error.code() {
        ErrorCode::InvalidArgument => "invalid-argument",
        ErrorCode::InvalidEncoding => "invalid-encoding",
        ErrorCode::Timeout => "timeout",
        ErrorCode::RuntimeError => "runtime-error",
        ErrorCode::UnsupportedOperation => "unsupported-operation",
        ErrorCode::TooLarge => "too-large",
        ErrorCode::NotFound => "not-found",
        ErrorCode::Security => "security",
        ErrorCode::Unknown => "unknown",
    };
    format!("depth: {what}: {code} ({})", error.data())
}

fn colour(color: Option<&ffrwd_node::ColorInfo>) -> Result<Colour, String> {
    Colour::of(color.map(|color| yuv::ColorInfo {
        range: &color.range,
        primaries: &color.primaries,
        trc: &color.trc,
        space: &color.space,
    }))
}

fn as_rgba<'a>(
    arrives: Arrives,
    bytes: &'a [u8],
    scratch: &'a mut [u8],
    width: usize,
    height: usize,
) -> Result<Rgba<'a>, String> {
    match arrives {
        Arrives::Rgba => Rgba::new(bytes, width, height),
        Arrives::Yuv420p(colour) => {
            yuv::to_rgba(&Yuv420p::new(bytes, width, height)?, colour, scratch)?;
            Rgba::new(scratch, width, height)
        }
    }
}

/// Where one square pixel reads from along an axis: the two frame samples it
/// falls between, and how far along it sits. Every square row reads the same
/// columns, so the column map is built once rather than per row.
struct Taps {
    low: Vec<usize>,
    high: Vec<usize>,
    fraction: Vec<f32>,
}

/// The map from `count` square steps back onto `source` frame samples.
fn taps(count: usize, source: usize, scale: f32) -> Taps {
    let mut map = Taps {
        low: Vec::with_capacity(count),
        high: Vec::with_capacity(count),
        fraction: Vec::with_capacity(count),
    };
    for step in 0..count {
        let f = ((step as f32 + 0.5) / scale - 0.5).clamp(0.0, (source - 1) as f32);
        let base = f.floor() as usize;
        map.low.push(base);
        map.high.push((base + 1).min(source - 1));
        map.fraction.push(f - base as f32);
    }
    map
}

/// One frame row as red, green and blue, a channel at a time so each is
/// contiguous.
fn row_to_rgb(frame: &Rgba, y: usize, out: &mut [f32]) {
    let width = frame.width;
    let (red, rest) = out.split_at_mut(width);
    let (green, blue) = rest.split_at_mut(width);
    for (x, pixel) in frame.data[y * width * 4..(y + 1) * width * 4]
        .as_chunks::<4>()
        .0
        .iter()
        .enumerate()
    {
        red[x] = f32::from(pixel[0]);
        green[x] = f32::from(pixel[1]);
        blue[x] = f32::from(pixel[2]);
    }
}

/// One frame row resized to the square's columns, channel by channel.
fn resize_row(rgb: &[f32], columns: &Taps, width: usize, out: &mut [f32]) {
    let count = columns.low.len();
    for channel in 0..3 {
        let source = &rgb[channel * width..(channel + 1) * width];
        let target = &mut out[channel * count..(channel + 1) * count];
        for (sx, sample) in target.iter_mut().enumerate() {
            let a = source[columns.low[sx]];
            let b = source[columns.high[sx]];
            *sample = a + (b - a) * columns.fraction[sx];
        }
    }
}

/// The frame scaled into the square the graph takes, normalized and laid out
/// as the planar fp32 tensor it expects. Everything outside the letterbox is
/// black, which the depth of the frame itself never reads back.
///
/// The resize is separable, so each frame row is turned into the square's
/// columns once and the two square rows that read it mix the same numbers.
/// Slots go by parity, and a square row mixes frame rows `y` and `y + 1`,
/// which never share one.
fn to_input(frame: &Rgba, box_: Letterbox) -> Vec<u8> {
    let (width, height) = (frame.width, frame.height);
    let plane = SIDE * SIDE;
    // Black, standardized, is where the padding sits.
    let mut planes = vec![0f32; plane * 3];
    for channel in 0..3 {
        let pad = (0.0 - MEAN[channel]) / STD[channel];
        planes[channel * plane..(channel + 1) * plane].fill(pad);
    }

    let columns = taps(box_.width, width, box_.scale);
    let rows = taps(box_.height, height, box_.scale);
    let mut rgb = vec![0f32; width * 3];
    let mut resized = [vec![0f32; box_.width * 3], vec![0f32; box_.width * 3]];
    let mut held: [Option<usize>; 2] = [None, None];

    for sy in 0..box_.height {
        for y in [rows.low[sy], rows.high[sy]] {
            let slot = y % 2;
            if held[slot] != Some(y) {
                row_to_rgb(frame, y, &mut rgb);
                resize_row(&rgb, &columns, width, &mut resized[slot]);
                held[slot] = Some(y);
            }
        }
        let (top_row, bottom_row) = (&resized[rows.low[sy] % 2], &resized[rows.high[sy] % 2]);
        let ty = rows.fraction[sy];

        let at = (box_.offset_y as usize + sy) * SIDE + box_.offset_x as usize;
        for channel in 0..3 {
            let (mean, std) = (MEAN[channel], STD[channel]);
            let top = &top_row[channel * box_.width..(channel + 1) * box_.width];
            let bottom = &bottom_row[channel * box_.width..(channel + 1) * box_.width];
            let target = &mut planes[channel * plane + at..channel * plane + at + box_.width];
            for ((sample, a), b) in target.iter_mut().zip(top).zip(bottom) {
                let value = (a + (b - a) * ty).clamp(0.0, 255.0) / 255.0;
                *sample = (value - mean) / std;
            }
        }
    }

    let mut bytes = vec![0u8; planes.len() * 4];
    let (words, _) = bytes.as_chunks_mut::<4>();
    for (word, value) in words.iter_mut().zip(&planes) {
        *word = value.to_le_bytes();
    }
    bytes
}

/// The graph's output as a plain grid of floats, whichever rank it came back
/// with: [1, h, w] as this graph spells it, or [1, 1, h, w] as an export that
/// keeps the channel does.
fn depth_grid(dimensions: &[u32], data: &[u8]) -> Result<(Vec<f32>, usize, usize), String> {
    let (h, w) = match dimensions {
        [_, h, w] => (*h as usize, *w as usize),
        [_, _, h, w] => (*h as usize, *w as usize),
        other => {
            return Err(format!(
                "depth: the graph returned a depth map of {} dimension(s), expected 3 or 4",
                other.len()
            ))
        }
    };
    let (whole, _) = data.as_chunks::<4>();
    let values: Vec<f32> = whole.iter().copied().map(f32::from_le_bytes).collect();
    if values.len() < h * w {
        return Err(format!(
            "depth: the graph returned {} value(s) for a {h}x{w} depth map",
            values.len()
        ));
    }
    Ok((values, h, w))
}

/// The depth map sampled back out to the frame's own size and scaled to fill
/// a byte. The range is the frame's own: the nearest thing in it is 255 and
/// the furthest 0, so a flat scene still uses the whole scale.
fn to_grey(
    grid: &[f32],
    grid_w: usize,
    grid_h: usize,
    width: usize,
    height: usize,
    box_: Letterbox,
) -> Vec<u8> {
    // The graph's grid may not be the square it was given, so square
    // coordinates are carried onto it by ratio.
    let gx_per_square = grid_w as f32 / SIDE as f32;
    let gy_per_square = grid_h as f32 / SIDE as f32;

    // Where each frame column and row reads from on the grid. Neither depends
    // on the other axis, so both are built once and the per-pixel work is
    // what is left: two rows of the grid mixed along their length.
    let place = |g: f32, extent: usize| -> (usize, usize, f32) {
        let g = g.clamp(0.0, (extent - 1) as f32);
        let base = g.floor() as usize;
        (base, (base + 1).min(extent - 1), g - base as f32)
    };
    let columns: Vec<(usize, usize, f32)> = (0..width)
        .map(|x| place(box_.to_square(x, 0).0 * gx_per_square, grid_w))
        .collect();

    // The range is taken over what the frame covers, so the black bars never
    // stretch it.
    let mut raw = vec![0f32; width * height];
    let (mut low, mut high) = (f32::INFINITY, f32::NEG_INFINITY);
    for y in 0..height {
        let (y0, y1, ty) = place(box_.to_square(0, y).1 * gy_per_square, grid_h);
        let top_row = &grid[y0 * grid_w..(y0 + 1) * grid_w];
        let bottom_row = &grid[y1 * grid_w..(y1 + 1) * grid_w];
        let target = &mut raw[y * width..(y + 1) * width];
        for (sample, (x0, x1, tx)) in target.iter_mut().zip(&columns) {
            let top = top_row[*x0] + (top_row[*x1] - top_row[*x0]) * tx;
            let bottom = bottom_row[*x0] + (bottom_row[*x1] - bottom_row[*x0]) * tx;
            let value = top + (bottom - top) * ty;
            *sample = value;
            low = low.min(value);
            high = high.max(value);
        }
    }

    let span = high - low;
    let mut out = vec![0u8; width * height];
    if span <= 0.0 {
        // A frame the model read as one flat distance. Mid grey says so,
        // rather than a scale that divides by nothing.
        out.fill(128);
        return out;
    }
    for (sample, value) in out.iter_mut().zip(&raw) {
        *sample = (((value - low) / span) * 255.0).round().clamp(0.0, 255.0) as u8;
    }
    out
}

impl Model {
    /// One frame through the graph.
    fn run(&self, frame: &Rgba) -> Result<Vec<u8>, String> {
        let input = to_input(frame, self.letterbox);
        let tensor = Tensor::new(&[1, 3, SIDE as u32, SIDE as u32], TensorType::Fp32, &input);
        let outputs = self
            .context
            .compute(vec![(INPUT_NAME.to_string(), tensor)])
            .map_err(|e| failed("compute", &e))?;

        let out = outputs
            .into_iter()
            .find(|(name, _)| name == OUTPUT_NAME)
            .map(|(_, tensor)| tensor)
            .ok_or_else(|| format!("depth: the graph returned no tensor named {OUTPUT_NAME}"))?;
        let (grid, grid_h, grid_w) = depth_grid(&out.dimensions(), &out.data())?;
        Ok(to_grey(
            &grid,
            grid_w,
            grid_h,
            frame.width,
            frame.height,
            self.letterbox,
        ))
    }
}

impl Node for Depth {
    const NAME: &'static str = "depth";
    const VERSION: &'static str = "0.2.0";
    type Params = NoParams;

    fn shape(_: &NoParams, _: &Bound) -> Result<Shape> {
        Ok(Shape::new()
            .input(
                Input::video("v")
                    .clock()
                    .pixel_formats(&["yuv420p", "rgba"]),
            )
            .output(Output::like("v").pixel_format("gray"))
            .pure()
            .one_to_one())
    }

    fn init(_: NoParams, init: &Init) -> Result<Depth> {
        let v = init.stream("v")?;
        let video = v
            .video_format()
            .ok_or("depth reads frames, and `v` is not video")?;
        let (width, height) = (video.width as usize, video.height as usize);
        let arrives = match video.pix_fmt.as_str() {
            "rgba" => Arrives::Rgba,
            "yuv420p" => Arrives::Yuv420p(colour(video.color.as_ref())?),
            other => return Err(format!("depth does not accept pixel format {other}").into()),
        };

        // The graph is loaded once per instance, and the session built once:
        // the first frame is what a provider picks its kernels on, and every
        // frame after it reuses them.
        let graph =
            load_by_name(MODEL).map_err(|e| failed(&format!("load-by-name({MODEL:?})"), &e))?;
        let context = graph
            .init_execution_context()
            .map_err(|e| failed("init-execution-context", &e))?;

        Ok(Depth {
            v: v.id,
            width,
            height,
            arrives,
            rgba: match arrives {
                Arrives::Rgba => Vec::new(),
                Arrives::Yuv420p(_) => vec![0; width * height * 4],
            },
            model: Model {
                letterbox: Letterbox::new(width, height),
                context,
                _graph: graph,
            },
        })
    }

    fn process(&mut self, tick: &Tick, out: &mut Out) -> Result<()> {
        for frame in tick.frames(self.v) {
            let bytes = tick.fetch(self.v, frame.index);
            let picture = as_rgba(
                self.arrives,
                &bytes,
                &mut self.rgba,
                self.width,
                self.height,
            )?;
            let map = self.model.run(&picture)?;
            out.frame("v", frame.pts, frame.duration, map)?;
        }
        Ok(())
    }
}

ffrwd_node::export!(Depth);

#[cfg(test)]
mod tests {
    use super::*;
    use ffrwd_node::{Format, Runner};

    fn read_f32(bytes: &[u8], index: usize) -> f32 {
        f32::from_le_bytes(bytes[index * 4..index * 4 + 4].try_into().expect("4 bytes"))
    }

    fn color(range: &str, space: &str) -> ffrwd_node::ColorInfo {
        ffrwd_node::ColorInfo {
            range: range.to_owned(),
            primaries: "unknown".to_owned(),
            trc: "unknown".to_owned(),
            space: space.to_owned(),
        }
    }

    fn flat_yuv(width: usize, height: usize, luma: u8) -> Vec<u8> {
        let mut frame = vec![128u8; Yuv420p::size(width, height)];
        frame[..width * height].fill(luma);
        frame
    }

    #[test]
    fn the_map_is_the_picture_in_gray() {
        let shape = Runner::<Depth>::shape("", &["v".to_owned()]).expect("a shape");
        assert_eq!(shape.clock_input(), Some("v"));
        assert_eq!(
            shape.inputs[0].accepts.pixel_formats,
            ["yuv420p", "rgba"],
            "yuv420p first, the smaller on the wire"
        );
        assert_eq!(shape.outputs.len(), 1);
        let map = &shape.outputs[0];
        assert_eq!(map.name, "v");
        assert_eq!(map.format, None::<Format>);
        let like = map.like.as_ref().expect("follows its input");
        assert_eq!(
            (like.port.as_deref(), like.pixel_format.as_deref()),
            (Some("v"), Some("gray"))
        );
        assert!(shape.pure && shape.one_to_one);
    }

    #[test]
    fn a_tv_range_picture_reaches_the_model_at_full_contrast() {
        // Black is 16 and white 235 on the wire. Read as full range, as this
        // module once did, they stayed 16 and 235; read as the stream's own
        // range they are 0 and 255.
        let tv = Arrives::Yuv420p(colour(Some(&color("tv", "bt470bg"))).expect("tv 601"));
        let untagged = Arrives::Yuv420p(colour(None).expect("unknown is tv 601"));
        assert_eq!(
            tv, untagged,
            "an untagged stream is read as swscale reads it"
        );
        for (luma, grey) in [(16u8, 0u8), (235, 255), (126, 128)] {
            let bytes = flat_yuv(4, 2, luma);
            let mut scratch = vec![0u8; 4 * 2 * 4];
            let picture = as_rgba(tv, &bytes, &mut scratch, 4, 2).expect("converts");
            assert!(
                picture
                    .data
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .all(|p| *p == [grey, grey, grey, 255]),
                "luma {luma} is grey {grey}: {:?}",
                &picture.data[..4]
            );
        }
    }

    #[test]
    fn a_full_range_picture_is_read_as_full_range() {
        let pc = Arrives::Yuv420p(colour(Some(&color("pc", "bt709"))).expect("pc 709"));
        let bytes = flat_yuv(2, 2, 235);
        let mut scratch = vec![0u8; 2 * 2 * 4];
        let picture = as_rgba(pc, &bytes, &mut scratch, 2, 2).expect("converts");
        assert_eq!(&picture.data[..4], [235, 235, 235, 255]);
    }

    #[test]
    fn a_matrix_ffrwd_frame_does_not_convert_is_refused_at_init() {
        let err = colour(Some(&color("tv", "ycgco"))).expect_err("not converted here");
        assert!(err.contains("rgba"), "{err}");
    }

    #[test]
    fn an_rgba_picture_is_taken_as_it_is() {
        let bytes: Vec<u8> = [9u8, 8, 7, 255].repeat(4);
        let picture = as_rgba(Arrives::Rgba, &bytes, &mut [], 2, 2).expect("rgba");
        assert_eq!(picture.data, &bytes[..]);
        assert!(as_rgba(Arrives::Rgba, &bytes[..12], &mut [], 2, 2).is_err());
    }

    #[test]
    fn a_wide_frame_is_letterboxed_with_bars_above_and_below() {
        let box_ = Letterbox::new(1024, 512);
        assert_eq!(box_.width, SIDE, "the wide side fills the square");
        // 512 frame pixels at 518/1024 to a pixel.
        assert_eq!(box_.height, 259, "and the other is scaled to fit");
        assert_eq!(box_.offset_x, 0.0);
        assert!(box_.offset_y > 0.0, "the bars are above and below");
    }

    #[test]
    fn a_square_frame_fills_the_square() {
        let box_ = Letterbox::new(256, 256);
        assert_eq!((box_.width, box_.height), (SIDE, SIDE));
        assert_eq!((box_.offset_x, box_.offset_y), (0.0, 0.0));
    }

    #[test]
    fn a_flat_frame_standardizes_to_one_value_per_channel() {
        // A 2x2 rgba frame of one colour: every tensor sample in a channel is
        // that channel's value rescaled to 0..1 and standardized.
        let frame: Vec<u8> = [128u8, 64, 32, 255].repeat(4);
        let rgba = Rgba::new(&frame, 2, 2).expect("2x2");
        let bytes = to_input(&rgba, Letterbox::new(2, 2));
        let plane = SIDE * SIDE;
        assert_eq!(bytes.len(), plane * 3 * 4);
        for (channel, value) in [128.0f32, 64.0, 32.0].iter().enumerate() {
            let expected = (value / 255.0 - MEAN[channel]) / STD[channel];
            for index in [0, plane / 2, plane - 1] {
                let sample = read_f32(&bytes, channel * plane + index);
                assert!(
                    (sample - expected).abs() < 1e-6,
                    "channel {channel} sample {index}: {sample} != {expected}"
                );
            }
        }
    }

    #[test]
    fn the_letterbox_bars_are_standardized_black() {
        // A wide frame leaves bars above and below; a bar sample is black run
        // through the same standardization the pixels get.
        let frame: Vec<u8> = [200u8, 200, 200, 255].repeat(4 * 2);
        let box_ = Letterbox::new(4, 2);
        let bytes = to_input(&Rgba::new(&frame, 4, 2).expect("4x2"), box_);
        let pad = (0.0 - MEAN[0]) / STD[0];
        let bar = read_f32(&bytes, 0);
        assert!(
            (bar - pad).abs() < 1e-6,
            "the top bar is standardized black"
        );
        let inside = read_f32(&bytes, box_.offset_y as usize * SIDE + SIDE / 2);
        let expected = (200.0 / 255.0 - MEAN[0]) / STD[0];
        assert!(
            (inside - expected).abs() < 1e-6,
            "and the frame's own rows are the pixels"
        );
    }

    #[test]
    fn a_depth_map_comes_back_from_three_dimensions_or_four() {
        let data: Vec<u8> = (0..6).flat_map(|v| (v as f32).to_le_bytes()).collect();
        let (three, h, w) = depth_grid(&[1, 2, 3], &data).expect("rank 3");
        assert_eq!((h, w), (2, 3));
        assert_eq!(three.len(), 6);
        let (four, h, w) = depth_grid(&[1, 1, 2, 3], &data).expect("rank 4");
        assert_eq!((h, w), (2, 3));
        assert_eq!(four, three, "the same grid, however it was shaped");
    }

    #[test]
    fn a_map_with_no_range_at_all_comes_out_mid_grey() {
        let flat = vec![7.0f32; SIDE * SIDE];
        let grey = to_grey(&flat, SIDE, SIDE, 4, 4, Letterbox::new(4, 4));
        assert!(
            grey.iter().all(|v| *v == 128),
            "one flat distance is no scale to normalize onto"
        );
    }

    #[test]
    fn the_range_is_stretched_to_fill_a_byte() {
        // A ramp across the square: whatever its raw values, the frame it
        // becomes runs from 0 to 255.
        let mut grid = vec![0f32; SIDE * SIDE];
        for y in 0..SIDE {
            for x in 0..SIDE {
                grid[y * SIDE + x] = 100.0 + x as f32 * 0.5;
            }
        }
        let grey = to_grey(&grid, SIDE, SIDE, 64, 64, Letterbox::new(64, 64));
        assert_eq!(grey.len(), 64 * 64, "one byte a pixel");
        assert_eq!(*grey.iter().min().expect("pixels"), 0);
        assert_eq!(*grey.iter().max().expect("pixels"), 255);
    }
}
