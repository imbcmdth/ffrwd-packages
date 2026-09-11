//! A sine the length of the stream it is handed: `frequency` Hz at amplitude
//! `level`, in the track's own rate and channel count, for exactly as many
//! samples as arrived. The samples of the input are never listened to - only
//! how many there are and when they start - so what leaves is a tone and
//! nothing of the audio it was cut to.
//!
//! It exists because a generated source cannot inherit a stream's length. A
//! bleep laid over a mask needs a tone as long as the track, and
//! `sine(duration => ...)` has to be told a number somebody worked out;
//! `tone(a)` is told by the stream.
//!
//! The phase carries from one call to the next, so the wave runs on across a
//! window boundary rather than restarting - which is what keeps the click out
//! of a tone assembled a window at a time.

// `generate_all`: `window-filter`'s records live in `ffrwd:av`'s own `types`
// interface, a second package the world never names directly.
wit_bindgen::generate!({
    path: ["wit", "wit-world"],
    world: "ffrwd:tone/tone",
    generate_all,
});

use std::cell::RefCell;
use std::f64::consts::TAU;

use exports::ffrwd::av::window_filter::{
    Format, FramePayload, Guest, InWindow, Meta, OutFrame, Processed, StreamInfo, WindowMeta,
};
use serde::Deserialize;

/// Samples one call carries, the same chunk `spans_mask` works in.
const WINDOW: u32 = 4096;

/// The one sample format this module writes, and the width of one value.
const SAMPLE_FMT: &str = "f32";
const SAMPLE_BYTES: usize = 4;

/// The highest frequency and level that mean anything: past half the rate a
/// sine is an alias of a lower one, and past unity it is clipping.
const MAX_FREQUENCY: f64 = 192_000.0;
const MAX_LEVEL: f64 = 1.0;

const PARAMS_SCHEMA: &str = r#"{"type":"object","properties":{"frequency":{"type":"number","exclusiveMinimum":0,"maximum":192000,"default":1000},"level":{"type":"number","minimum":0,"maximum":1,"default":0.3}},"additionalProperties":false}"#;

/// The censor's own pitch: well inside speech's band, so it covers what it
/// replaces, and high enough to be heard as a mark rather than a hum.
fn default_frequency() -> f64 {
    1000.0
}

/// Loud enough to stand for the speech it replaces, quiet enough not to be
/// the loudest thing in the mix.
fn default_level() -> f64 {
    0.3
}

#[derive(Clone, Copy, Deserialize)]
// The schema says these two and no others, and this is what makes that true.
#[serde(deny_unknown_fields)]
struct Params {
    #[serde(default = "default_frequency")]
    frequency: f64,
    #[serde(default = "default_level")]
    level: f64,
}

impl Default for Params {
    fn default() -> Params {
        Params {
            frequency: default_frequency(),
            level: default_level(),
        }
    }
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

/// What `init` settled.
struct Opened {
    oscillator: Oscillator,
}

thread_local! {
    static OPENED: RefCell<Option<Opened>> = const { RefCell::new(None) };
}

fn parse_params(params: &str) -> Result<Params, String> {
    let trimmed = params.trim();
    let parsed: Params = if trimmed.is_empty() {
        Params::default()
    } else {
        serde_json::from_str(trimmed).map_err(|e| format!("tone cannot read its params: {e}"))?
    };
    if !parsed.frequency.is_finite() || parsed.frequency <= 0.0 || parsed.frequency > MAX_FREQUENCY
    {
        return Err(format!(
            "tone needs frequency in hertz above 0 and no more than {MAX_FREQUENCY}, got {}",
            parsed.frequency
        ));
    }
    if !parsed.level.is_finite() || !(0.0..=MAX_LEVEL).contains(&parsed.level) {
        return Err(format!(
            "tone needs level between 0 and {MAX_LEVEL}, got {}",
            parsed.level
        ));
    }
    Ok(parsed)
}

struct Tone;

impl Guest for Tone {
    fn describe() -> WindowMeta {
        WindowMeta {
            meta: Meta {
                name: "tone".to_string(),
                version: "0.1.0".to_string(),
                params_schema: PARAMS_SCHEMA.to_string(),
                rows_schema: String::new(),
                // An audio module, so it names no pixel formats.
                pixel_formats: vec![],
                sample_formats: vec![SAMPLE_FMT.to_string()],
                // The tone takes the track's own rate and channel count,
                // whatever they are, so neither is narrowed here.
                sample_rates: vec![],
                channel_counts: vec![],
                rows_language: vec![],
            },
            window: WINDOW,
            stride: WINDOW,
            // The phase carries from one call to the next.
            pure: false,
            // One payload out for every payload in, at its own pts and its
            // own length: the tone is the input's timeline exactly, which is
            // the whole point of taking a stream it never listens to.
            one_to_one: true,
            // Whatever arrived with the samples is nothing to a sine.
            reads_rows: false,
            forwards_rows: false,
            // One stream in: the audio the tone is cut to.
            inputs: 1,
        }
    }

    fn init(format: Format, _stream_info: StreamInfo, params: String) -> Result<(), String> {
        let Format::Audio(audio) = format else {
            return Err("tone writes samples, and this stream is video".to_string());
        };
        if audio.sample_fmt != SAMPLE_FMT {
            return Err(format!(
                "tone does not accept sample format {}",
                audio.sample_fmt
            ));
        }
        let parsed = parse_params(&params)?;

        OPENED.with(|o| {
            *o.borrow_mut() = Some(Opened {
                oscillator: Oscillator::new(
                    f64::from(audio.sample_rate),
                    audio.channels as usize,
                    parsed,
                ),
            });
        });
        Ok(())
    }

    fn set_params(params: String) -> Result<(), String> {
        let parsed = parse_params(&params)?;
        OPENED.with(|o| {
            if let Some(opened) = o.borrow_mut().as_mut() {
                // The phase stands: a new frequency turns the wave faster
                // from here rather than starting it again.
                opened.oscillator.params = parsed;
            }
        });
        Ok(())
    }

    fn process(window: &InWindow, _trailing: Vec<String>, _last: bool) -> Processed {
        OPENED.with(|o| {
            let mut borrowed = o.borrow_mut();
            let opened = borrowed
                .as_mut()
                .expect("init settles the format before any audio arrives");
            let width = opened.oscillator.channels * SAMPLE_BYTES;

            let mut frames = Vec::with_capacity(window.len() as usize);
            for index in 0..window.len() {
                // The samples are fetched for their COUNT alone - the tone is
                // as long as the audio it was cut to, and the window says a
                // payload's size no other way.
                let samples = window.fetch(index).len() / width;
                frames.push(OutFrame {
                    pts: window.pts(index),
                    frame: FramePayload::New(opened.oscillator.draw(samples)),
                    rows: vec![],
                });
            }
            Processed {
                frames,
                trailing: vec![],
            }
        })
    }
}

export!(Tone);

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(parse_params(r#"{"frequency":0}"#).is_err());
        assert!(parse_params(r#"{"frequency":-440}"#).is_err());
        assert!(parse_params(r#"{"level":1.5}"#).is_err());
        assert!(parse_params(r#"{"level":-0.1}"#).is_err());
        assert!(parse_params(r#"{"pitch":440}"#).is_err());
    }

    #[test]
    fn no_params_at_all_are_the_defaults() {
        for written in ["", "{}", "  "] {
            let parsed = parse_params(written).expect("the defaults");
            assert_eq!((parsed.frequency, parsed.level), (1000.0, 0.3));
        }
    }

    #[test]
    fn each_parameter_can_be_set_on_its_own() {
        let parsed = parse_params(r#"{"frequency":440}"#).expect("one of them");
        assert_eq!(parsed.frequency, 440.0);
        assert_eq!(parsed.level, 0.3, "the rest keep their defaults");
    }
}
