//! Where one shot ends and the next begins. Each frame's luma is reduced to a
//! small grid of cell averages and compared with the previous frame's: a mean
//! absolute difference above `threshold` is a cut, and the shot counter steps.
//! The first frame is shot 0.
//!
//! Every frame leaves one row on `cuts`, `{"start_t": s, "shot": n}`, and
//! nothing else leaves: a reader takes the picture from its source. `start_t`
//! is the time of the shot's first frame, the same on every row of one shot,
//! so `ffrwd.merge_spans` turns the rows into the shots themselves, each
//! ending where its last frame does. A shot one frame long is that frame
//! long.
//!
//! The luma-grid rule: cheap, and blind to a cut a colour grade alone would
//! make. Room stays beside it in this package for a better detector later.

use ffrwd_node::{Bound, Init, Input, Node, Out, Output, Result, Shape, Tick};
use serde::{Deserialize, Serialize};

const PARAMS_SCHEMA: &str = r#"{"type":"object","properties":{"threshold":{"type":"number","exclusiveMinimum":0,"default":12.0}},"additionalProperties":false}"#;

/// Cells a frame is reduced to, per side. Small enough that a moving subject
/// barely moves the average, large enough that a new picture moves all of them.
const GRID: usize = 32;

#[derive(Deserialize)]
struct Params {
    threshold: f64,
}

#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
struct Cut {
    start_t: f64,
    shot: i64,
}

/// The pixel format the host chose at `init`, fixed for the instance's life.
#[derive(Clone, Copy, PartialEq)]
enum PixFmt {
    Yuv420p,
    Rgba,
}

struct SimpleDetector {
    v: u32,
    width: usize,
    height: usize,
    pix_fmt: PixFmt,
    threshold: f64,
    /// The previous frame's cells, absent until a frame has been seen.
    previous: Option<Vec<u8>>,
    /// The current frame's cells, reused every frame.
    cells: Vec<u8>,
    shot: i64,
    start_t: f64,
}

/// The luma of one run of one row, totalled. yuv420p carries luma in the
/// first plane; rgba is converted with the standard weights, per pixel, so
/// each one rounds where it always did.
fn row_luma_sum(
    frame: &[u8],
    pix_fmt: PixFmt,
    width: usize,
    y: usize,
    x0: usize,
    x1: usize,
) -> u32 {
    match pix_fmt {
        PixFmt::Yuv420p => frame[y * width + x0..y * width + x1]
            .iter()
            .map(|sample| u32::from(*sample))
            .sum(),
        PixFmt::Rgba => {
            let base = (y * width + x0) * 4;
            frame[base..base + (x1 - x0) * 4]
                .as_chunks::<4>()
                .0
                .iter()
                .map(|pixel| {
                    let r = u32::from(pixel[0]);
                    let g = u32::from(pixel[1]);
                    let b = u32::from(pixel[2]);
                    (r * 299 + g * 587 + b * 114) / 1000
                })
                .sum()
        }
    }
}

/// The frame's luma as `GRID`x`GRID` cell averages, written into `cells`. A
/// frame smaller than the grid repeats pixels rather than leaving cells empty.
///
/// A cell's columns are one contiguous run of each of its rows, so it is
/// totalled a run at a time and the format is decided once per run rather
/// than once per pixel.
fn downsample(frame: &[u8], pix_fmt: PixFmt, width: usize, height: usize, cells: &mut Vec<u8>) {
    cells.clear();
    cells.resize(GRID * GRID, 0);
    let columns: Vec<(usize, usize)> = (0..GRID)
        .map(|cx| {
            let x0 = cx * width / GRID;
            (x0, ((cx + 1) * width / GRID).max(x0 + 1))
        })
        .collect();

    for cy in 0..GRID {
        let y0 = cy * height / GRID;
        let y1 = ((cy + 1) * height / GRID).max(y0 + 1);
        for (cx, (x0, x1)) in columns.iter().enumerate() {
            let mut total: u32 = 0;
            for y in y0..y1 {
                total += row_luma_sum(frame, pix_fmt, width, y, *x0, *x1);
            }
            let count = ((y1 - y0) * (x1 - x0)) as u32;
            cells[cy * GRID + cx] = (total / count.max(1)) as u8;
        }
    }
}

/// Mean absolute difference between two frames' cells, in luma steps.
fn mean_abs_diff(a: &[u8], b: &[u8]) -> f64 {
    if a.is_empty() {
        return 0.0;
    }
    let total: u32 = a
        .iter()
        .zip(b)
        .map(|(p, q)| u32::from(p.abs_diff(*q)))
        .sum();
    f64::from(total) / a.len() as f64
}

impl SimpleDetector {
    fn see(&mut self, frame: &[u8], t: f64) -> Cut {
        let mut cells = std::mem::take(&mut self.cells);
        downsample(frame, self.pix_fmt, self.width, self.height, &mut cells);
        match &self.previous {
            Some(previous) if mean_abs_diff(previous, &cells) > self.threshold => {
                self.shot += 1;
                self.start_t = t;
            }
            Some(_) => {}
            None => self.start_t = t,
        }
        // These cells become the previous frame's; the ones they replace go
        // back to being the scratch buffer.
        self.cells = self.previous.replace(cells).unwrap_or_default();
        Cut {
            start_t: self.start_t,
            shot: self.shot,
        }
    }
}

impl Node for SimpleDetector {
    const NAME: &'static str = "simple_detector";
    const VERSION: &'static str = "0.2.0";
    const PARAMS_SCHEMA: &'static str = PARAMS_SCHEMA;
    type Params = Params;

    fn shape(_: &Params, _: &Bound) -> Result<Shape> {
        Ok(Shape::new()
            .input(
                Input::video("v")
                    .clock()
                    .pixel_formats(&["yuv420p", "rgba"]),
            )
            .output(Output::rows("cuts").schema::<Cut>()))
    }

    fn init(params: Params, init: &Init) -> Result<SimpleDetector> {
        let v = init.stream("v")?;
        let video = v
            .video_format()
            .ok_or("simple_detector reads pictures, and `v` is not video")?;
        let pix_fmt = match video.pix_fmt.as_str() {
            "yuv420p" => PixFmt::Yuv420p,
            "rgba" => PixFmt::Rgba,
            other => return Err(format!("unsupported pix-fmt: {other:?}").into()),
        };
        Ok(SimpleDetector {
            v: v.id,
            width: video.width as usize,
            height: video.height as usize,
            pix_fmt,
            threshold: params.threshold,
            previous: None,
            cells: Vec::with_capacity(GRID * GRID),
            shot: 0,
            start_t: 0.0,
        })
    }

    fn set_params(&mut self, params: Params) -> Result<()> {
        self.threshold = params.threshold;
        Ok(())
    }

    fn process(&mut self, tick: &Tick, out: &mut Out) -> Result<()> {
        let time_base = tick.time_base();
        for frame in tick.frames(self.v) {
            let pixels = tick.fetch(self.v, frame.index);
            let cut = self.see(&pixels, time_base.seconds(frame.pts));
            out.row("cuts", frame.pts, &cut)?;
        }
        Ok(())
    }
}

ffrwd_node::export!(SimpleDetector);

#[cfg(test)]
mod tests {
    use super::*;
    use ffrwd_node::mock::Harness;
    use ffrwd_node::{BoundStream, Format, Rational};

    /// A `width`x`height` rgba frame filled with one grey level.
    fn flat_rgba(width: usize, height: usize, level: u8) -> Vec<u8> {
        let mut frame = vec![255u8; width * height * 4];
        for pixel in frame.chunks_mut(4) {
            pixel[0] = level;
            pixel[1] = level;
            pixel[2] = level;
        }
        frame
    }

    fn detector(params: &str) -> Harness<SimpleDetector> {
        let v = BoundStream::video("v", 0, 64, 64, "rgba", Rational::new(1, 15));
        Harness::new(params, vec![v]).expect("opens")
    }

    fn rows(detector: &mut Harness<SimpleDetector>, first: i64, levels: &[u8]) -> Vec<(i64, Cut)> {
        let mut rows = Vec::new();
        for (n, level) in levels.iter().enumerate() {
            let pts = first + n as i64;
            let tick = detector.tick(pts).frame(0, pts, flat_rgba(64, 64, *level));
            let emitted = detector.process(&tick).expect("processes");
            for (pts, json) in emitted.messages("cuts") {
                rows.push((pts, ffrwd_node::parse(&json).expect("a row")));
            }
        }
        rows
    }

    #[test]
    fn a_flat_frame_reduces_to_one_level_in_every_cell() {
        let mut cells = Vec::new();
        downsample(&flat_rgba(64, 64, 90), PixFmt::Rgba, 64, 64, &mut cells);
        assert_eq!(cells.len(), GRID * GRID);
        assert!(
            cells.iter().all(|c| *c == 90),
            "every cell of a flat frame holds the frame's level"
        );
    }

    #[test]
    fn a_frame_smaller_than_the_grid_still_fills_it() {
        let mut cells = Vec::new();
        downsample(&flat_rgba(8, 8, 40), PixFmt::Rgba, 8, 8, &mut cells);
        assert_eq!(cells.len(), GRID * GRID, "cells repeat rather than vanish");
    }

    #[test]
    fn the_difference_is_the_luma_step_between_two_flat_frames() {
        let mut dark = Vec::new();
        let mut light = Vec::new();
        downsample(&flat_rgba(64, 64, 10), PixFmt::Rgba, 64, 64, &mut dark);
        downsample(&flat_rgba(64, 64, 210), PixFmt::Rgba, 64, 64, &mut light);
        assert!((mean_abs_diff(&dark, &light) - 200.0).abs() < 1e-9);
        assert_eq!(mean_abs_diff(&dark, &dark), 0.0);
    }

    #[test]
    fn a_threshold_of_zero_or_less_is_refused_by_value() {
        for bad in ["0", "-1.5"] {
            let params = format!(r#"{{"threshold":{bad}}}"#);
            let Err(err) = ffrwd_node::read_params::<Params>(PARAMS_SCHEMA, &params) else {
                panic!("a threshold of {bad} should have been refused");
            };
            assert!(err.contains("threshold"), "got: {err}");
        }
        let (params, _) =
            ffrwd_node::read_params::<Params>(PARAMS_SCHEMA, "").expect("no params is the default");
        assert_eq!(params.threshold, 12.0);
    }

    #[test]
    fn rows_alone_leave_the_detector() {
        let detector = detector("");
        let shape = detector.shape();
        assert_eq!(shape.clock_input(), Some("v"));
        assert_eq!(shape.outputs.len(), 1, "no picture comes back out");
        let cuts = shape.find_output("cuts").expect("the rows");
        assert_eq!(cuts.format, Some(Format::Data("json".to_owned())));
        let schema = cuts.schema.as_deref().expect("a schema");
        assert!(
            schema.contains(r#""start_t":{"type":"number"}"#),
            "{schema}"
        );
        assert!(schema.contains(r#""shot":{"type":"integer"}"#), "{schema}");
    }

    #[test]
    fn every_frame_of_a_shot_carries_the_time_it_began() {
        let mut detector = detector("");
        let rows = rows(&mut detector, 30, &[10, 10, 210, 210, 210]);
        let seen: Vec<(i64, i64, f64)> = rows
            .iter()
            .map(|(pts, cut)| (*pts, cut.shot, cut.start_t))
            .collect();
        assert_eq!(
            seen,
            [
                (30, 0, 2.0),
                (31, 0, 2.0),
                (32, 1, 32.0 / 15.0),
                (33, 1, 32.0 / 15.0),
                (34, 1, 32.0 / 15.0)
            ],
            "the first shot starts at the first frame, not at 0"
        );
    }

    #[test]
    fn a_shot_one_frame_long_starts_on_its_frame_and_ends_at_the_next() {
        // A flash between two cuts: the shot it makes is one frame, whose row
        // is the only one naming its start, and the shot after starts on the
        // following frame. Merged, it runs one frame's duration; read as the
        // span from its first frame to its last, it was zero-length.
        let mut detector = detector("");
        let rows = rows(&mut detector, 0, &[10, 10, 210, 10, 10]);
        let shots: Vec<i64> = rows.iter().map(|(_, cut)| cut.shot).collect();
        assert_eq!(shots, [0, 0, 1, 2, 2]);
        let starts: Vec<f64> = rows.iter().map(|(_, cut)| cut.start_t).collect();
        assert_eq!(starts, [0.0, 0.0, 2.0 / 15.0, 3.0 / 15.0, 3.0 / 15.0]);
        let flash: Vec<i64> = rows
            .iter()
            .filter(|(_, cut)| cut.start_t == 2.0 / 15.0)
            .map(|(pts, _)| *pts)
            .collect();
        assert_eq!(flash, [2], "one row, on the flash itself");
    }

    #[test]
    fn a_new_threshold_holds_from_the_next_frame() {
        let mut detector = detector("");
        assert_eq!(rows(&mut detector, 0, &[10, 30])[1].1.shot, 1);
        detector
            .set_params(r#"{"threshold":50}"#)
            .expect("a threshold above 0");
        assert_eq!(
            rows(&mut detector, 2, &[60])[0].1.shot,
            1,
            "30 is no cut at 50"
        );
        assert!(detector.set_params(r#"{"threshold":0}"#).is_err());
    }
}
