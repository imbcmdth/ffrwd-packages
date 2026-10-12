//! `blur_where` and `mosaic_where` on the sidecar's device: `v` filtered
//! where the matte marks it, in compute passes, with the frames held on the
//! GPU from the decoder to the encoder where `memory` is `gpu`.
//!
//! Two inputs in lockstep: `v`, the clock, and `matte`, conformed to `v`'s
//! size, both rgba, the matte's weight its red channel. One output like
//! `v`, rgba. `effect` picks the filter: `blur`, a Gaussian of `sigma`
//! (radius `ceil(3 * sigma)`, weights normalised, edges clamped), or
//! `mosaic`, the average of each `size` by `size` block from the top-left
//! corner, truncated, the blocks at the right and bottom edges as large as
//! what is left of the frame, which is ffmpeg's `pixelize`. Then
//! `out = v + (filtered - v) * matte / 255`, rounded to bytes, opaque, with
//! the filtered picture rounded to bytes first: a matte of 255 gives the
//! filtered picture exactly and a matte of 0 gives `v` exactly.
//!
//! `memory` says where the frames are held. On `gpu` both inputs and the
//! output are textures of the device and nothing crosses to host memory.
//! On `host`, the default, the module uploads both frames, runs the same
//! passes, and reads the result back, so the two give the same picture.
//!
//! The pipelines, their layouts and every intermediate are made once at
//! `init`, since the size is fixed; a frame is bind groups and dispatches.

use ffrwd_node::{Bound, Init, Input, Node, Out, Output, Result, Shape, Tick};
use serde::Deserialize;

#[cfg(target_arch = "wasm32")]
mod gpu;

#[cfg(any(target_arch = "wasm32", test))]
mod shaders;

/// On any target but wasm32 there is no device: the node's shape and params
/// are tested on the host, and `init` says why it stops.
#[cfg(not(target_arch = "wasm32"))]
mod gpu {
    use crate::{Memory, Plan};

    pub struct Device;

    impl Device {
        pub fn open(_plan: &Plan, _memory: Memory) -> Result<Device, String> {
            Err("masked_gpu: the passes run on the sidecar's device, \
                 which only the wasm32 module reaches"
                .to_owned())
        }

        pub fn on_device(&self, _v: (u32, u32), _matte: (u32, u32)) -> Result<Texture, String> {
            unreachable!("no device is opened off wasm32")
        }

        pub fn on_host(&self, _v: &[u8], _matte: &[u8]) -> Result<Vec<u8>, String> {
            unreachable!("no device is opened off wasm32")
        }
    }

    pub struct Texture;

    pub fn emit(
        _port: &str,
        _pts: i64,
        _duration: Option<i64>,
        _texture: Texture,
    ) -> Result<(), String> {
        unreachable!("no device is opened off wasm32")
    }
}

const PARAMS_SCHEMA: &str = r#"{"type":"object","properties":{"effect":{"type":"string","enum":["blur","mosaic"]},"sigma":{"type":"number","minimum":0,"maximum":1024,"default":12},"size":{"type":"integer","minimum":1,"maximum":1024,"default":16},"memory":{"type":"string","enum":["host","gpu"],"default":"host"}},"required":["effect"],"additionalProperties":false}"#;

/// The one pixel format either input is taken in, and the output's.
const RGBA: &str = "rgba";

/// The output port.
const OUT: &str = "out";

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Effect {
    Blur,
    Mosaic,
}

/// Where the frames are held.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Memory {
    Host,
    Gpu,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
pub struct Params {
    effect: Effect,
    sigma: f64,
    size: u32,
    memory: Memory,
}

fn read(params: &str) -> std::result::Result<Params, String> {
    ffrwd_node::read_params::<Params>(PARAMS_SCHEMA, params).map(|(params, _)| params)
}

/// The ports held on the GPU for these params, as `ports.memory` answers:
/// every video port on `gpu`, none on `host` or on params that do not read.
pub fn held_ports(params: &str) -> &'static [&'static str] {
    match read(params) {
        Ok(Params {
            memory: Memory::Gpu,
            ..
        }) => &["v", "matte", OUT],
        _ => &[],
    }
}

/// The Gaussian's weights from the centre out: `radius + 1` of them, the
/// radius `ceil(3 * sigma)`, normalised so the whole kernel (each weight but
/// the centre counted twice) sums to 1. A sigma of 0 is the one weight 1.
pub fn gaussian(sigma: f64) -> Vec<f32> {
    let radius = (3.0 * sigma).ceil() as usize;
    if radius == 0 {
        return vec![1.0];
    }
    let raw: Vec<f64> = (0..=radius)
        .map(|k| (-((k * k) as f64) / (2.0 * sigma * sigma)).exp())
        .collect();
    let total = raw[0] + 2.0 * raw[1..].iter().sum::<f64>();
    raw.iter().map(|w| (w / total) as f32).collect()
}

/// The composite of one byte, as the passes compute it:
/// `base + (filtered - base) * m / 255` rounded to the nearest byte.
pub fn blend(base: u8, filtered: u8, m: u8) -> u8 {
    let (b, f, m) = (u32::from(base), u32::from(filtered), u32::from(m));
    ((b * (255 - m) + f * m + 127) / 255) as u8
}

/// What the passes are made for: the effect, the frame's size, and what
/// the effect needs of it.
pub struct Plan {
    pub effect: Effect,
    pub width: u32,
    pub height: u32,
    /// The blur's weights from the centre out; empty for the mosaic.
    pub weights: Vec<f32>,
    /// The mosaic's block side; 1 for the blur.
    pub size: u32,
}

impl Plan {
    pub fn new(params: &Params, width: u32, height: u32) -> Plan {
        let (weights, size) = match params.effect {
            Effect::Blur => (gaussian(params.sigma), 1),
            Effect::Mosaic => (Vec::new(), params.size),
        };
        Plan {
            effect: params.effect,
            width,
            height,
            weights,
            size,
        }
    }

    pub fn radius(&self) -> u32 {
        self.weights.len().saturating_sub(1) as u32
    }

    /// Mosaic blocks across and down, the last of each as large as what is
    /// left.
    pub fn blocks(&self) -> (u32, u32) {
        (
            self.width.div_ceil(self.size),
            self.height.div_ceil(self.size),
        )
    }

    /// The passes' `Params` uniform, as [`shaders::COMMON`] lays it out.
    pub fn uniform(&self) -> Vec<u8> {
        let (blocks_x, blocks_y) = self.blocks();
        [
            self.width,
            self.height,
            self.radius(),
            self.size,
            blocks_x,
            blocks_y,
            0,
            0,
        ]
        .iter()
        .flat_map(|word| word.to_le_bytes())
        .collect()
    }
}

struct MaskedGpu {
    v: u32,
    matte: u32,
    memory: Memory,
    device: gpu::Device,
}

/// `port`'s frame size, refusing a stream that is not rgba video.
fn rgba_size(init: &Init, port: &str) -> Result<(u32, u32)> {
    let stream = init.stream(port)?;
    let video = stream
        .video_format()
        .ok_or_else(|| format!("masked_gpu: `{port}` is video"))?;
    if video.pix_fmt != RGBA {
        return Err(format!(
            "masked_gpu: `{port}` is taken in rgba, and arrived as {}",
            video.pix_fmt
        )
        .into());
    }
    Ok((video.width, video.height))
}

impl Node for MaskedGpu {
    const NAME: &'static str = "masked_gpu";
    const VERSION: &'static str = "0.1.0";
    const PARAMS_SCHEMA: &'static str = PARAMS_SCHEMA;
    type Params = Params;

    fn shape(_: &Params, _: &Bound) -> Result<Shape> {
        Ok(Shape::new()
            .input(Input::video("v").clock().pixel_formats(&[RGBA]))
            .input(
                Input::video("matte")
                    .lockstep()
                    .like("v")
                    .pixel_formats(&[RGBA]),
            )
            .output(Output::video(OUT).following("v").pixel_format(RGBA))
            .pure()
            .one_to_one())
    }

    fn init(params: Params, init: &Init) -> Result<MaskedGpu> {
        let (width, height) = rgba_size(init, "v")?;
        let matte_size = rgba_size(init, "matte")?;
        if matte_size != (width, height) {
            return Err(format!(
                "masked_gpu: the matte is {}x{} and `v` {width}x{height}; it follows v's size",
                matte_size.0, matte_size.1
            )
            .into());
        }
        let device = gpu::Device::open(&Plan::new(&params, width, height), params.memory)?;
        Ok(MaskedGpu {
            v: init.stream("v")?.id,
            matte: init.stream("matte")?.id,
            memory: params.memory,
            device,
        })
    }

    fn process(&mut self, tick: &Tick, out: &mut Out) -> Result<()> {
        let mattes = tick.frames(self.matte);
        for (at, frame) in tick.frames(self.v).into_iter().enumerate() {
            let matte = mattes
                .get(at)
                .ok_or("masked_gpu: a frame of `v` came without its frame of the matte")?;
            match self.memory {
                Memory::Gpu => {
                    let texture = self
                        .device
                        .on_device((self.v, frame.index), (self.matte, matte.index))?;
                    gpu::emit(OUT, frame.pts, frame.duration, texture)?;
                }
                Memory::Host => {
                    let bytes = self.device.on_host(
                        &tick.fetch(self.v, frame.index),
                        &tick.fetch(self.matte, matte.index),
                    )?;
                    out.frame(OUT, frame.pts, frame.duration, bytes)?;
                }
            }
        }
        Ok(())
    }
}

ffrwd_node::export!(MaskedGpu);

#[cfg(test)]
mod tests {
    use super::*;
    use ffrwd_node::mock::Harness;
    use ffrwd_node::{BoundStream, Kind, Pairing, Rational, Runner};

    fn stream(port: &str, id: u32, width: u32, height: u32, pix_fmt: &str) -> BoundStream {
        BoundStream::video(port, id, width, height, pix_fmt, Rational::new(1, 30))
    }

    fn opened(params: &str, matte: (u32, u32), pix_fmt: &str) -> String {
        let bound = vec![
            stream("v", 0, 64, 48, RGBA),
            stream("matte", 1, matte.0, matte.1, pix_fmt),
        ];
        match Harness::<MaskedGpu>::new(params, bound) {
            Ok(_) => panic!("there is no device off wasm32"),
            Err(e) => e,
        }
    }

    #[test]
    fn the_defaults_are_ffmpeg_s_and_host_memory() {
        let parsed = read(r#"{"effect":"blur"}"#).expect("reads");
        assert_eq!(
            parsed,
            Params {
                effect: Effect::Blur,
                sigma: 12.0,
                size: 16,
                memory: Memory::Host
            }
        );
        let parsed = read(r#"{"effect":"mosaic","size":8.0,"memory":"gpu"}"#).expect("reads");
        assert_eq!((parsed.effect, parsed.size), (Effect::Mosaic, 8));
        assert_eq!(parsed.memory, Memory::Gpu);
    }

    #[test]
    fn params_outside_the_schema_are_refused() {
        for written in [
            "",
            r#"{"sigma":3}"#,
            r#"{"effect":"smear"}"#,
            r#"{"effect":"blur","sigma":-1}"#,
            r#"{"effect":"blur","sigma":2000}"#,
            r#"{"effect":"mosaic","size":0}"#,
            r#"{"effect":"mosaic","size":2.5}"#,
            r#"{"effect":"mosaic","size":2048}"#,
            r#"{"effect":"blur","memory":"disk"}"#,
            r#"{"effect":"blur","radius":3}"#,
        ] {
            assert!(read(written).is_err(), "{written} should be refused");
        }
    }

    #[test]
    fn gpu_memory_holds_every_video_port_and_host_none() {
        assert_eq!(
            held_ports(r#"{"effect":"blur","memory":"gpu"}"#),
            ["v", "matte", "out"]
        );
        assert!(held_ports(r#"{"effect":"mosaic","memory":"host"}"#).is_empty());
        assert!(
            held_ports(r#"{"effect":"mosaic"}"#).is_empty(),
            "host is the default"
        );
        assert!(
            held_ports(r#"{"memory":"gpu"}"#).is_empty(),
            "params that do not read"
        );
    }

    #[test]
    fn the_shape_is_the_same_in_either_memory() {
        let bound = [
            stream("v", 0, 64, 48, RGBA),
            stream("matte", 1, 64, 48, RGBA),
        ];
        let host =
            Runner::<MaskedGpu>::shape(r#"{"effect":"blur"}"#, Bound::of(&bound)).expect("shapes");
        let gpu =
            Runner::<MaskedGpu>::shape(r#"{"effect":"mosaic","memory":"gpu"}"#, Bound::of(&bound))
                .expect("shapes");
        assert_eq!(host, gpu);

        let [v, matte] = &host.inputs[..] else {
            panic!("two inputs: {:?}", host.inputs)
        };
        assert_eq!((v.name.as_str(), v.kind, v.clock), ("v", Kind::Video, true));
        assert_eq!(
            (matte.name.as_str(), matte.kind, matte.clock),
            ("matte", Kind::Video, false)
        );
        assert_eq!(matte.pairing, Pairing::Lockstep);
        assert_eq!(
            matte.accepts.like.as_deref(),
            Some("v"),
            "the matte follows v's size"
        );
        for input in [v, matte] {
            assert_eq!(input.accepts.pixel_formats, ["rgba"]);
            assert_eq!((input.window, input.stride), (1, 1));
        }

        let [out] = &host.outputs[..] else {
            panic!("one output: {:?}", host.outputs)
        };
        assert_eq!((out.name.as_str(), out.kind), ("out", Kind::Video));
        let like = out.like.as_ref().expect("follows v");
        assert_eq!(
            (like.port.as_deref(), like.pixel_format.as_deref()),
            (Some("v"), Some("rgba"))
        );
        assert!(host.pure && host.one_to_one);
        assert_eq!(host.clock_input(), Some("v"));
    }

    #[test]
    fn init_refuses_a_matte_of_another_size_or_format_before_the_device() {
        let blur = r#"{"effect":"blur"}"#;
        assert!(opened(blur, (32, 48), RGBA).contains("32x48"));
        assert!(opened(blur, (64, 48), "yuv420p").contains("yuv420p"));
        assert!(
            opened(blur, (64, 48), RGBA).contains("device"),
            "a well-formed call gets as far as opening the device"
        );
    }

    #[test]
    fn the_kernel_is_three_sigmas_each_way_and_sums_to_one() {
        for sigma in [0.5, 1.0, 2.3, 12.0, 40.0] {
            let weights = gaussian(sigma);
            assert_eq!(weights.len() - 1, (3.0 * sigma).ceil() as usize);
            let total = f64::from(weights[0])
                + 2.0 * weights[1..].iter().map(|&w| f64::from(w)).sum::<f64>();
            assert!((total - 1.0).abs() < 1e-5, "sigma {sigma}: {total}");
            assert!(weights.windows(2).all(|w| w[1] < w[0]), "falls off");
        }
        assert_eq!(gaussian(0.0), [1.0], "a sigma of 0 leaves the picture");
    }

    #[test]
    fn a_kernel_weight_is_the_gaussian_s() {
        let weights = gaussian(2.0);
        let ratio = f64::from(weights[2]) / f64::from(weights[0]);
        assert!((ratio - (-0.5f64).exp()).abs() < 1e-6, "{ratio}");
    }

    #[test]
    fn a_full_matte_is_the_filtered_byte_and_none_the_base() {
        for base in [0u8, 1, 77, 128, 254, 255] {
            for filtered in [0u8, 3, 128, 200, 255] {
                assert_eq!(blend(base, filtered, 255), filtered);
                assert_eq!(blend(base, filtered, 0), base);
            }
        }
    }

    #[test]
    fn a_partial_matte_rounds_to_the_nearest_byte() {
        for base in 0..=255u8 {
            for m in [1u8, 64, 127, 128, 200, 254] {
                let filtered = 255 - base;
                let exact = f64::from(base)
                    + (f64::from(filtered) - f64::from(base)) * f64::from(m) / 255.0;
                let got = blend(base, filtered, m);
                assert!(
                    (f64::from(got) - exact).abs() <= 0.5,
                    "{base} to {filtered} at {m}: {got} for {exact}"
                );
            }
        }
    }

    #[test]
    fn the_plan_counts_partial_blocks_at_the_edges() {
        let mosaic = read(r#"{"effect":"mosaic","size":16}"#).unwrap();
        let plan = Plan::new(&mosaic, 1280, 720);
        assert_eq!(plan.blocks(), (80, 45));
        let plan = Plan::new(&mosaic, 100, 37);
        assert_eq!(
            plan.blocks(),
            (7, 3),
            "16, 16 ... 4 across and 16, 16, 5 down"
        );
        assert_eq!(plan.radius(), 0);

        let blur = read(r#"{"effect":"blur","sigma":12}"#).unwrap();
        let plan = Plan::new(&blur, 1280, 720);
        assert_eq!((plan.radius(), plan.size), (36, 1));
    }

    #[test]
    fn the_uniform_is_eight_words_in_the_shader_s_order() {
        let mosaic = read(r#"{"effect":"mosaic","size":16}"#).unwrap();
        let words: Vec<u32> = Plan::new(&mosaic, 100, 37)
            .uniform()
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| u32::from_le_bytes(*b))
            .collect();
        assert_eq!(words, [100, 37, 0, 16, 7, 3, 0, 0]);
    }

    #[test]
    fn every_pass_validates() {
        for (name, pass) in [
            ("blur rows", shaders::BLUR_ROWS),
            ("blur columns", shaders::BLUR_COLUMNS),
            ("mosaic rows", shaders::MOSAIC_ROWS),
            ("mosaic blocks", shaders::MOSAIC_BLOCKS),
            ("mosaic composite", shaders::MOSAIC_COMPOSITE),
        ] {
            let source = shaders::source(pass);
            let module = naga::front::wgsl::parse_str(&source)
                .unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(&source)));
            naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::empty(),
            )
            .validate(&module)
            .unwrap_or_else(|e| panic!("{name}: {e:?}"));
            assert_eq!(module.entry_points.len(), 1, "{name}");
        }
    }
}
