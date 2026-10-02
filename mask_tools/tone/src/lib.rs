//! A sine the length of the audio it is cut to: `frequency` Hz at amplitude
//! `level`, f32 in that audio's own rate and channel count, one run of samples
//! for every run that arrives and at its time. The samples that arrive are
//! never fetched: a tick says how many there are and when they start, and
//! that is all a tone needs of them.
//!
//! It exists because a generated source cannot inherit a stream's length. A
//! bleep laid over a mask needs a tone as long as the track, and
//! `sine(duration => ...)` has to be told a number somebody worked out;
//! `tone(a)` is told by the stream.
//!
//! The phase carries from one tick to the next, so the wave runs on across a
//! window boundary rather than restarting - which is what keeps the click out
//! of a tone assembled a window at a time.

use std::f64::consts::TAU;

use ffrwd_node::{Bound, Init, Input, Node, Out, Output, Rational, Result, Shape, Tick};
use serde::Deserialize;

/// Samples one tick carries, the same chunk `spans_mask` works in.
const WINDOW: u32 = 4096;

/// The one sample format this module writes, and the width of one value.
const SAMPLE_FMT: &str = "f32";
const SAMPLE_BYTES: usize = 4;

const PARAMS_SCHEMA: &str = r#"{"type":"object","properties":{"frequency":{"type":"number","exclusiveMinimum":0,"maximum":192000,"default":1000},"level":{"type":"number","minimum":0,"maximum":1,"default":0.3}},"additionalProperties":false}"#;

#[derive(Clone, Copy, Deserialize)]
struct Params {
    frequency: f64,
    level: f64,
}

/// The wave being drawn: where it is, and how far it turns per sample.
struct Oscillator {
    rate: f64,
    channels: usize,
    params: Params,
    /// Radians into the wave, wrapped each sample so it never drifts.
    phase: f64,
}

impl Oscillator {
    fn new(rate: f64, channels: usize, params: Params) -> Oscillator {
        Oscillator {
            rate,
            channels,
            params,
            phase: 0.0,
        }
    }

    /// `samples` more of the wave, the same value in every channel, carrying
    /// on from wherever the last call left off.
    fn draw(&mut self, samples: usize) -> Vec<u8> {
        let step = TAU * self.params.frequency / self.rate;
        let mut out = Vec::with_capacity(samples * self.channels * SAMPLE_BYTES);
        for _ in 0..samples {
            let bytes = ((self.params.level * self.phase.sin()) as f32).to_le_bytes();
            for _ in 0..self.channels {
                out.extend_from_slice(&bytes);
            }
            self.phase = (self.phase + step) % TAU;
        }
        out
    }
}

struct Tone {
    a: u32,
    counts_samples: bool,
    width: usize,
    oscillator: Oscillator,
}

impl Node for Tone {
    const NAME: &'static str = "tone";
    const VERSION: &'static str = "0.2.0";
    const PARAMS_SCHEMA: &'static str = PARAMS_SCHEMA;
    type Params = Params;

    fn shape(_: &Params, _: &Bound) -> Result<Shape> {
        Ok(Shape::new()
            .input(Input::audio("a").clock().window(WINDOW, WINDOW))
            .output(Output::like("a").sample_format(SAMPLE_FMT))
            .one_to_one())
    }

    fn init(params: Params, init: &Init) -> Result<Tone> {
        let a = init.stream("a")?;
        let audio = a
            .audio_format()
            .ok_or("tone is cut to audio, and `a` is not audio")?;
        let sample_bytes = match audio.sample_fmt.as_str() {
            "f32" => 4,
            "s16" => 2,
            other => return Err(format!("tone does not accept sample format {other}").into()),
        };
        let channels = audio.channels as usize;
        Ok(Tone {
            a: a.id,
            counts_samples: a.info.time_base == Rational::new(1, audio.sample_rate as i32),
            width: sample_bytes * channels,
            oscillator: Oscillator::new(f64::from(audio.sample_rate), channels, params),
        })
    }

    fn set_params(&mut self, params: Params) -> Result<()> {
        // The phase stands: a new frequency turns the wave faster from here
        // rather than starting it again.
        self.oscillator.params = params;
        Ok(())
    }

    fn process(&mut self, tick: &Tick, out: &mut Out) -> Result<()> {
        for frame in tick.frames(self.a) {
            let samples = match frame.duration {
                Some(duration) if self.counts_samples => duration as usize,
                // A duration in any other time base was rounded on its way
                // there; the bytes are the count exactly.
                _ => tick.fetch(self.a, frame.index).len() / self.width,
            };
            out.frame(
                "a",
                frame.pts,
                frame.duration,
                self.oscillator.draw(samples),
            )?;
        }
        Ok(())
    }
}

ffrwd_node::export!(Tone);

#[cfg(test)]
mod tests {
    use super::*;
    use ffrwd_node::mock::Harness;
    use ffrwd_node::{BoundStream, Payload};

    const RATE: f64 = 8000.0;

    fn params(frequency: f64, level: f64) -> Params {
        Params { frequency, level }
    }

    /// A drawn payload's values, one per sample of one channel.
    fn values(drawn: &[u8], channels: usize) -> Vec<f32> {
        let (whole, _) = drawn.as_chunks::<4>();
        whole
            .iter()
            .copied()
            .map(f32::from_le_bytes)
            .step_by(channels)
            .collect()
    }

    fn read(written: &str) -> std::result::Result<Params, String> {
        ffrwd_node::read_params::<Params>(PARAMS_SCHEMA, written).map(|(params, _)| params)
    }

    fn made(tone: &mut Harness<Tone>, tick: ffrwd_node::mock::Tick) -> (i64, Option<i64>, Vec<u8>) {
        let emitted = tone.process(&tick).expect("processes");
        let [Payload::Frame {
            pts,
            duration,
            data,
        }] = &emitted.on("a")[..]
        else {
            panic!("one run out per run in: {emitted:?}")
        };
        (*pts, *duration, data.clone())
    }

    #[test]
    fn the_tone_follows_its_audio_in_f32() {
        let a = BoundStream::audio("a", 0, 44_100, 1, "s16");
        let tone = Harness::<Tone>::new("", vec![a]).expect("opens");
        let shape = tone.shape();
        let input = &shape.inputs[0];
        assert_eq!((input.window, input.stride), (WINDOW, WINDOW));
        assert!(
            input.accepts.sample_formats.is_empty(),
            "never read, so any"
        );
        let like = shape.outputs[0].like.as_ref().expect("follows `a`");
        assert_eq!(
            (like.port.as_deref(), like.sample_format.as_deref()),
            (Some("a"), Some("f32"))
        );
        assert!(shape.one_to_one && !shape.pure);
    }

    #[test]
    fn a_run_s_length_comes_from_its_duration_and_its_samples_stay_unread() {
        let a = BoundStream::audio("a", 0, 48_000, 2, "f32");
        let mut tone = Harness::<Tone>::new("", vec![a]).expect("opens");
        // The run's bytes are empty here: a tone that fetched them would draw
        // nothing.
        let tick = tone
            .tick(4096)
            .frame_with(0, 4096, Some(4096), &[], Vec::new());
        let (pts, duration, data) = made(&mut tone, tick);
        assert_eq!((pts, duration), (4096, Some(4096)));
        assert_eq!(data.len(), 4096 * 2 * 4, "stereo, f32");
    }

    #[test]
    fn a_run_counted_in_another_time_base_is_measured_by_its_bytes() {
        let mut a = BoundStream::audio("a", 0, 48_000, 1, "s16");
        a.info.time_base = Rational::new(1, 1000);
        let mut tone = Harness::<Tone>::new("", vec![a]).expect("opens");
        let tick = tone
            .tick(0)
            .frame_with(0, 0, Some(85), &[], vec![0; 4096 * 2]);
        let (_, _, data) = made(&mut tone, tick);
        assert_eq!(data.len(), 4096 * 4, "4096 samples, not 85 ms of them");
    }

    #[test]
    fn the_tone_is_as_long_as_the_audio_it_was_cut_to() {
        let mut oscillator = Oscillator::new(RATE, 2, params(1000.0, 0.3));
        assert_eq!(oscillator.draw(100).len(), 100 * 2 * 4, "stereo, f32");
        assert_eq!(oscillator.draw(0).len(), 0, "and an empty call draws none");
    }

    #[test]
    fn every_channel_carries_the_same_wave() {
        let mut oscillator = Oscillator::new(RATE, 2, params(1000.0, 0.5));
        let drawn = oscillator.draw(8);
        let (whole, _) = drawn.as_chunks::<4>();
        let samples: Vec<f32> = whole.iter().copied().map(f32::from_le_bytes).collect();
        for pair in samples.as_chunks::<2>().0 {
            assert_eq!(pair[0], pair[1]);
        }
    }

    #[test]
    fn the_wave_is_a_sine_at_the_frequency_asked_for() {
        // 1000 Hz at 8000 Hz is eight samples a cycle, so the quarter turn
        // lands on the second sample and the half turn on the fourth.
        let mut oscillator = Oscillator::new(RATE, 1, params(1000.0, 1.0));
        let drawn = values(&oscillator.draw(8), 1);
        assert!(drawn[0].abs() < 1e-6, "the wave starts at zero");
        assert!((drawn[2] - 1.0).abs() < 1e-6, "a quarter turn is the peak");
        assert!(drawn[4].abs() < 1e-6, "a half turn is zero again");
        assert!(
            (drawn[6] + 1.0).abs() < 1e-6,
            "three quarters is the trough"
        );
    }

    #[test]
    fn level_is_the_amplitude() {
        let mut oscillator = Oscillator::new(RATE, 1, params(1000.0, 0.25));
        let drawn = values(&oscillator.draw(8), 1);
        assert!((drawn[2] - 0.25).abs() < 1e-6);
        assert!(drawn.iter().all(|v| v.abs() <= 0.25 + 1e-6));
    }

    #[test]
    fn the_phase_runs_on_across_two_windows() {
        // One oscillator drawing two windows and one drawing the whole
        // stretch agree sample for sample: nothing restarts at the boundary.
        let mut split = Oscillator::new(RATE, 1, params(700.0, 0.4));
        let mut whole = Oscillator::new(RATE, 1, params(700.0, 0.4));
        let mut drawn = values(&split.draw(37), 1);
        drawn.extend(values(&split.draw(43), 1));
        let once = values(&whole.draw(80), 1);
        for (index, (a, b)) in drawn.iter().zip(&once).enumerate() {
            assert!((a - b).abs() < 1e-6, "sample {index}: {a} against {b}");
        }
    }

    #[test]
    fn the_phase_stays_inside_one_turn() {
        let mut oscillator = Oscillator::new(RATE, 1, params(1000.0, 0.3));
        oscillator.draw(10_000);
        assert!(
            (0.0..TAU).contains(&oscillator.phase),
            "a long stream does not drift the phase out of range"
        );
    }

    #[test]
    fn params_outside_the_schema_are_refused_by_name() {
        assert!(read(r#"{"frequency":0}"#).is_err());
        assert!(read(r#"{"frequency":-440}"#).is_err());
        assert!(read(r#"{"frequency":200000}"#).is_err());
        assert!(read(r#"{"level":1.5}"#).is_err());
        assert!(read(r#"{"level":-0.1}"#).is_err());
        assert!(read(r#"{"pitch":440}"#).is_err());
    }

    #[test]
    fn no_params_at_all_are_the_defaults() {
        for written in ["", "{}", "  "] {
            let parsed = read(written).expect("the defaults");
            assert_eq!((parsed.frequency, parsed.level), (1000.0, 0.3));
        }
    }

    #[test]
    fn each_parameter_can_be_set_on_its_own() {
        let parsed = read(r#"{"frequency":440}"#).expect("one of them");
        assert_eq!(parsed.frequency, 440.0);
        assert_eq!(parsed.level, 0.3, "the rest keep their defaults");
    }
}
