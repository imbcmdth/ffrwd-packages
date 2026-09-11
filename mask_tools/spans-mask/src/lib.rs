//! Cues to a mask: the spans arriving as rows beside the samples become an
//! audio stream at the track's own rate and channel count - 1.0 inside a
//! span, 0 outside, and a linear ramp between where `feather` asks for one.
//! `grow` pads every span outward in milliseconds; `feather` softens each
//! edge over that many, falling from the grown span's edge to nothing.
//!
//! The samples themselves are never listened to - only how many arrived and
//! when - so the mask can be multiplied back against the track by whatever
//! consumes a mask beside it. A row that is not a cue - one an upstream
//! module emitted for something else - is skipped rather than refused.
//!
//! # Why the audio is held back
//!
//! A recognizer's rows arrive AFTER the audio they describe: a row rides
//! the window its span CLOSED on, so it reaches this module a window after
//! the span's END and speaks for every sample back to the span's start. So
//! this module cannot decide a sample when it arrives. It holds each payload
//! instead and emits it - at its ORIGINAL pts - once `lag` seconds of LATER
//! audio have been seen, by which time every row that can still speak for it
//! has arrived. The final call flushes whatever is left, however little
//! audio followed it.
//!
//! What `lag` has to outlast is therefore the longest SPAN, not the
//! recognizer's own latency: a two-second lag against a VAD whose windows
//! are a second wide still leaves a four-second sentence masked from its
//! last second on, because the row that names it arrives when the mask for
//! its first three seconds has already been written.
//!
//! That is what the declaration says. `one_to_one` is false: a call's output
//! is not the call's own input, the first calls of the stream emit nothing at
//! all, and what leaves is a mask rather than the samples that arrived.
//! `pure` is false: the held payloads and the spans told so far both carry
//! from one call to the next. `window` is `stride`, so the windows are
//! disjoint and tile the stream, which is what lets the mask that leaves be
//! as long as the audio that arrived, sample for sample. Downstream, the
//! ffmpeg filter reading the mask waits for it the way `maskedmerge` waits on
//! a slow detector today.

// `generate_all`: `window-filter`'s records live in `ffrwd:av`'s own `types`
// interface, a second package the world never names directly.
wit_bindgen::generate!({
    path: ["wit", "wit-world"],
    world: "ffrwd:spans-mask/spans-mask",
    generate_all,
});

use std::cell::RefCell;
use std::collections::VecDeque;

use exports::ffrwd::av::window_filter::{
    Format, FramePayload, Guest, InWindow, Meta, OutFrame, Processed, StreamInfo, WindowMeta,
};
use serde::Deserialize;

/// Samples one call carries. A tenth of a second at 48 kHz: fine enough that
/// the flush at the end of a stream costs nothing, coarse enough that a
/// stream's worth of calls is cheap.
const WINDOW: u32 = 4096;

/// The one sample format this module writes, and the width of one value.
const SAMPLE_FMT: &str = "f32";
const SAMPLE_BYTES: usize = 4;

/// Milliseconds in a second, which is the unit `grow` and `feather` are in.
const MS: f64 = 1000.0;

/// The largest `grow` or `feather` that means anything, in milliseconds, and
/// the largest `lag`, in seconds.
const MAX_MS: f64 = 60_000.0;
const MAX_LAG: f64 = 60.0;

const PARAMS_SCHEMA: &str = r#"{"type":"object","properties":{"grow":{"type":"number","minimum":0,"maximum":60000,"default":0},"feather":{"type":"number","minimum":0,"maximum":60000,"default":0},"lag":{"type":"number","minimum":0,"maximum":60,"default":32}},"additionalProperties":false}"#;

/// Seconds of later audio a payload waits for. What it has to outlast is the
/// longest span a recognizer will report, since a span's row rides the window
/// it closed on and reaches back to the span's start - so this is a length of
/// SPEECH, not a latency. Half a minute covers an uninterrupted paragraph and
/// a transcriber's widest window both, and costs what it holds: one second of
/// 48 kHz stereo f32 is 384 KB, so the wait is 12 MB.
fn default_lag() -> f64 {
    32.0
}

#[derive(Clone, Copy, Deserialize)]
// The schema says these three and no others, and this is what makes that true.
#[serde(deny_unknown_fields)]
struct Params {
    #[serde(default)]
    grow: f64,
    #[serde(default)]
    feather: f64,
    #[serde(default = "default_lag")]
    lag: f64,
}

impl Default for Params {
    fn default() -> Params {
        Params {
            grow: 0.0,
            feather: 0.0,
            lag: default_lag(),
        }
    }
}

/// One span to mask, as an upstream recognizer reports it. Extra keys are
/// ignored: a cue carrying the text it heard beside its two bounds is still a
/// span.
#[derive(Deserialize)]
struct Cue {
    start_t: f64,
    end_t: f64,
}

/// One span in seconds from the start of the stream, as it was told.
#[derive(Clone, Copy, PartialEq)]
struct Span {
    start_t: f64,
    end_t: f64,
}

/// One payload waiting for the audio behind it.
#[derive(Clone, Copy)]
struct Held {
    /// The timestamp it arrived at, which is the one it leaves at.
    pts: i64,
    start_t: f64,
    samples: usize,
}

impl Held {
    fn end_t(&self, rate: f64) -> f64 {
        self.start_t + self.samples as f64 / rate
    }
}

/// One payload on its way out: the mask for the audio it stands for.
struct Emitted {
    pts: i64,
    samples: Vec<u8>,
}

/// The spans told so far and the payloads still waiting on them.
struct Masker {
    rate: f64,
    channels: usize,
    params: Params,
    spans: Vec<Span>,
    held: VecDeque<Held>,
    /// Where the newest audio seen ends, in seconds.
    seen_t: f64,
    /// Where the mask written so far ends, in seconds.
    emitted_t: f64,
}

impl Masker {
    fn new(rate: f64, channels: usize, params: Params) -> Masker {
        Masker {
            rate,
            channels,
            params,
            spans: Vec::new(),
            held: VecDeque::new(),
            seen_t: f64::NEG_INFINITY,
            emitted_t: f64::NEG_INFINITY,
        }
    }

    /// The rows arriving with one payload. A row that is not a cue, and a
    /// span that ends before it starts, are skipped; a span already told is
    /// not told twice.
    fn learn(&mut self, rows: &[String]) {
        for row in rows {
            let Ok(cue) = serde_json::from_str::<Cue>(row) else {
                continue;
            };
            let span = Span {
                start_t: cue.start_t,
                end_t: cue.end_t,
            };
            if !span.start_t.is_finite() || !span.end_t.is_finite() || span.end_t < span.start_t {
                continue;
            }
            if !self.spans.contains(&span) {
                self.spans.push(span);
            }
        }
    }

    /// One payload, kept until the audio behind it has been seen.
    fn hold(&mut self, pts: i64, start_t: f64, samples: usize) {
        let held = Held {
            pts,
            start_t,
            samples,
        };
        self.seen_t = self.seen_t.max(held.end_t(self.rate));
        self.held.push_back(held);
    }

    /// Whatever `lag` seconds of later audio have now settled, oldest first.
    /// `last` is the end of the stream, which settles everything left.
    fn release(&mut self, last: bool) -> Vec<Emitted> {
        let settled = self.seen_t - self.params.lag;
        let mut out = Vec::new();
        while let Some(held) = self.held.front().copied() {
            if !last && held.end_t(self.rate) > settled {
                break;
            }
            self.held.pop_front();
            out.push(Emitted {
                pts: held.pts,
                samples: self.paint(&held),
            });
            self.emitted_t = held.end_t(self.rate);
        }
        // A span the mask has already been written past can say nothing more.
        let reach = (self.params.grow + self.params.feather) / MS;
        let emitted_t = self.emitted_t;
        self.spans.retain(|span| span.end_t + reach >= emitted_t);
        out
    }

    /// One payload's worth of mask, the same value in every channel.
    fn paint(&self, held: &Held) -> Vec<u8> {
        let mut out = Vec::with_capacity(held.samples * self.channels * SAMPLE_BYTES);
        for index in 0..held.samples {
            let at = held.start_t + index as f64 / self.rate;
            let bytes = self.coverage(at).to_le_bytes();
            for _ in 0..self.channels {
                out.extend_from_slice(&bytes);
            }
        }
        out
    }

    /// The mask at one instant: the strongest cover any span gives it, so
    /// overlapping spans union rather than add.
    fn coverage(&self, at: f64) -> f32 {
        let (grow, feather) = (self.params.grow / MS, self.params.feather / MS);
        let mut best: f64 = 0.0;
        for span in &self.spans {
            best = best.max(alpha(at, span.start_t - grow, span.end_t + grow, feather));
            if best >= 1.0 {
                break;
            }
        }
        best as f32
    }
}

/// The mask at `at` for one span: full inside `[edge0, edge1]`, falling
/// linearly to nothing `feather` seconds outside either edge.
fn alpha(at: f64, edge0: f64, edge1: f64, feather: f64) -> f64 {
    let outside = (edge0 - at).max(at - edge1).max(0.0);
    if outside <= 0.0 {
        return 1.0;
    }
    if feather <= 0.0 {
        return 0.0;
    }
    (1.0 - outside / feather).max(0.0)
}

/// What `init` settled.
struct Opened {
    /// The unit this stream's timestamps are counted in.
    time_base: (i32, i32),
    masker: Masker,
}

thread_local! {
    static OPENED: RefCell<Option<Opened>> = const { RefCell::new(None) };
}

fn parse_params(params: &str) -> Result<Params, String> {
    let trimmed = params.trim();
    let parsed: Params = if trimmed.is_empty() {
        Params::default()
    } else {
        serde_json::from_str(trimmed)
            .map_err(|e| format!("spans_mask cannot read its params: {e}"))?
    };
    for (name, value) in [("grow", parsed.grow), ("feather", parsed.feather)] {
        if !value.is_finite() || !(0.0..=MAX_MS).contains(&value) {
            return Err(format!(
                "spans_mask needs {name} in milliseconds between 0 and {MAX_MS}, got {value}"
            ));
        }
    }
    if !parsed.lag.is_finite() || !(0.0..=MAX_LAG).contains(&parsed.lag) {
        return Err(format!(
            "spans_mask needs lag in seconds between 0 and {MAX_LAG}, got {}",
            parsed.lag
        ));
    }
    Ok(parsed)
}

/// A timestamp in the stream's own unit, as seconds. `den` is always
/// positive, so a negative timestamp stays negative.
fn seconds(ticks: i64, num: i32, den: i32) -> f64 {
    ticks as f64 * f64::from(num) / f64::from(den)
}

struct SpansMask;

impl Guest for SpansMask {
    fn describe() -> WindowMeta {
        WindowMeta {
            meta: Meta {
                name: "spans_mask".to_string(),
                version: "0.1.0".to_string(),
                params_schema: PARAMS_SCHEMA.to_string(),
                rows_schema: String::new(),
                // An audio module, so it names no pixel formats.
                pixel_formats: vec![],
                sample_formats: vec![SAMPLE_FMT.to_string()],
                // The mask takes the track's own rate and channel count,
                // whatever they are, so neither is narrowed here.
                sample_rates: vec![],
                channel_counts: vec![],
                rows_language: vec![],
            },
            window: WINDOW,
            stride: WINDOW,
            // The payloads held back and the spans told so far both carry
            // from one call to the next.
            pure: false,
            // A payload leaves `lag` seconds of audio after the call that
            // brought it, and what leaves is a mask rather than the samples
            // that arrived.
            one_to_one: false,
            // The rows are the spans this module masks.
            reads_rows: true,
            // And they are consumed by it: what leaves is the mask alone.
            forwards_rows: false,
            // One stream in: the audio the mask is cut to.
            inputs: 1,
        }
    }

    fn init(format: Format, stream_info: StreamInfo, params: String) -> Result<(), String> {
        let Format::Audio(audio) = format else {
            return Err("spans_mask masks samples, and this stream is video".to_string());
        };
        if audio.sample_fmt != SAMPLE_FMT {
            return Err(format!(
                "spans_mask does not accept sample format {}",
                audio.sample_fmt
            ));
        }
        let parsed = parse_params(&params)?;

        OPENED.with(|o| {
            *o.borrow_mut() = Some(Opened {
                time_base: (stream_info.time_base.num, stream_info.time_base.den),
                masker: Masker::new(
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
                // The spans stand: they are what an upstream module found,
                // not something these numbers decided.
                opened.masker.params = parsed;
            }
        });
        Ok(())
    }

    fn process(window: &InWindow, trailing: Vec<String>, last: bool) -> Processed {
        OPENED.with(|o| {
            let mut borrowed = o.borrow_mut();
            let opened = borrowed
                .as_mut()
                .expect("init settles the format before any audio arrives");
            let (num, den) = opened.time_base;
            let width = opened.masker.channels * SAMPLE_BYTES;

            for index in 0..window.len() {
                let pts = window.pts(index);
                opened.masker.learn(&window.rows(index));
                // The samples are fetched for their COUNT alone - the mask
                // has to be as long as the audio it stands for, and the
                // window says a payload's size no other way.
                let samples = window.fetch(index).len() / width;
                opened.masker.hold(pts, seconds(pts, num, den), samples);
            }
            // Rows the last call had nothing to put them on still name spans.
            opened.masker.learn(&trailing);

            let frames = opened
                .masker
                .release(last)
                .into_iter()
                .map(|emitted| OutFrame {
                    pts: emitted.pts,
                    frame: FramePayload::New(emitted.samples),
                    // The spans were the rows' whole purpose; none travel on.
                    rows: vec![],
                })
                .collect();
            Processed {
                frames,
                trailing: vec![],
            }
        })
    }
}

export!(SpansMask);

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f64 = 1000.0;

    fn params(grow: f64, feather: f64, lag: f64) -> Params {
        Params { grow, feather, lag }
    }

    fn masker(channels: usize, params: Params) -> Masker {
        Masker::new(RATE, channels, params)
    }

    fn rows(items: &[&str]) -> Vec<String> {
        items.iter().map(|r| r.to_string()).collect()
    }

    /// One cue row, in seconds.
    fn cue(start_t: f64, end_t: f64) -> String {
        format!(r#"{{"text":"speech","start_t":{start_t},"end_t":{end_t}}}"#)
    }

    /// An emitted payload's mask values, one per sample of one channel.
    fn values(emitted: &Emitted, channels: usize) -> Vec<f32> {
        let (whole, _) = emitted.samples.as_chunks::<4>();
        whole
            .iter()
            .copied()
            .map(f32::from_le_bytes)
            .step_by(channels)
            .collect()
    }

    /// Feeds one second of audio a payload at a time, ten payloads a second,
    /// and answers everything that left.
    fn run(masker: &mut Masker, payloads: usize, told: &[(usize, String)]) -> Vec<Emitted> {
        let per = RATE as usize / 10;
        let mut out = Vec::new();
        for index in 0..payloads {
            let at = index * per;
            let said: Vec<String> = told
                .iter()
                .filter(|(on, _)| *on == index)
                .map(|(_, row)| row.clone())
                .collect();
            masker.learn(&said);
            masker.hold(at as i64, at as f64 / RATE, per);
            out.extend(masker.release(index + 1 == payloads));
        }
        out
    }

    #[test]
    fn a_span_masks_its_own_seconds_and_nothing_else() {
        let mut masker = masker(1, params(0.0, 0.0, 0.0));
        masker.learn(&rows(&[&cue(0.2, 0.4)]));
        masker.hold(0, 0.0, 1000);
        let out = masker.release(true);
        let mask = values(&out[0], 1);
        assert_eq!(mask[100], 0.0, "before the span");
        assert_eq!(mask[200], 1.0, "the first sample of the span");
        assert_eq!(mask[300], 1.0, "inside it");
        assert_eq!(mask[400], 1.0, "the last sample of the span");
        assert_eq!(mask[401], 0.0, "one past it is background");
    }

    #[test]
    fn grow_pads_the_span_outward() {
        let mut masker = masker(1, params(50.0, 0.0, 0.0));
        masker.learn(&rows(&[&cue(0.2, 0.4)]));
        masker.hold(0, 0.0, 1000);
        let out = masker.release(true);
        let mask = values(&out[0], 1);
        assert_eq!(mask[152], 1.0, "fifty milliseconds before the span");
        assert_eq!(mask[448], 1.0, "and fifty after it");
        assert_eq!(mask[148], 0.0, "past the growth is background");
        assert_eq!(mask[452], 0.0);
    }

    #[test]
    fn feather_ramps_linearly_outside_the_grown_span() {
        let mut masker = masker(1, params(50.0, 100.0, 0.0));
        masker.learn(&rows(&[&cue(0.3, 0.5)]));
        masker.hold(0, 0.0, 1000);
        let out = masker.release(true);
        let mask = values(&out[0], 1);
        assert_eq!(mask[260], 1.0, "the grown span itself is full");
        assert_eq!(mask[540], 1.0);
        // 100 ms of ramp below the grown edge at 0.25 s.
        assert!((mask[200] - 0.5).abs() < 1e-6, "half way out is half");
        assert!(
            (mask[225] - 0.75).abs() < 1e-6,
            "a quarter out is three quarters"
        );
        assert!((mask[175] - 0.25).abs() < 1e-6);
        assert_eq!(mask[140], 0.0, "past the feather is background");
        // And the same ramp above the grown edge at 0.55 s.
        assert!((mask[600] - 0.5).abs() < 1e-6);
        assert_eq!(mask[660], 0.0);
    }

    #[test]
    fn overlapping_spans_union_rather_than_add() {
        let mut masker = masker(1, params(0.0, 0.0, 0.0));
        masker.learn(&rows(&[&cue(0.1, 0.3), &cue(0.2, 0.5)]));
        masker.hold(0, 0.0, 1000);
        let out = masker.release(true);
        let mask = values(&out[0], 1);
        assert_eq!(mask[250], 1.0, "the overlap is full, not doubled");
        assert_eq!(mask[150], 1.0);
        assert_eq!(mask[450], 1.0);
        assert_eq!(mask[50], 0.0);
        assert_eq!(mask[550], 0.0);
    }

    #[test]
    fn the_mask_carries_the_same_value_in_every_channel() {
        let mut masker = masker(2, params(0.0, 0.0, 0.0));
        masker.learn(&rows(&[&cue(0.0, 1.0)]));
        masker.hold(0, 0.0, 10);
        let out = masker.release(true);
        assert_eq!(out[0].samples.len(), 10 * 2 * 4, "ten stereo samples");
        let (whole, _) = out[0].samples.as_chunks::<4>();
        assert!(whole.iter().copied().all(|b| f32::from_le_bytes(b) == 1.0));
    }

    #[test]
    fn a_payload_waits_for_lag_seconds_of_later_audio() {
        let mut masker = masker(1, params(0.0, 0.0, 0.3));
        // Ten payloads of 100 ms. The first four settle only once the audio
        // reaching 0.7 s has arrived.
        let out = run(&mut masker, 10, &[]);
        assert_eq!(out.len(), 10, "everything leaves by the end of the stream");
        let pts: Vec<i64> = out.iter().map(|e| e.pts).collect();
        assert_eq!(pts, (0..10).map(|i| i * 100).collect::<Vec<_>>());
    }

    #[test]
    fn nothing_leaves_before_lag_seconds_have_followed_it() {
        let mut masker = masker(1, params(0.0, 0.0, 0.3));
        for index in 0..3 {
            let at = index * 100;
            masker.hold(at, at as f64 / RATE, 100);
            assert!(
                masker.release(false).is_empty(),
                "0.3 s of audio has not followed the first payload yet"
            );
        }
        masker.hold(300, 0.3, 100);
        let out = masker.release(false);
        assert_eq!(out.len(), 1, "the first payload has 0.3 s behind it now");
        assert_eq!(out[0].pts, 0, "and it leaves at the pts it arrived at");
    }

    #[test]
    fn the_last_call_flushes_everything_still_held() {
        let mut masker = masker(1, params(0.0, 0.0, 10.0));
        for index in 0..5 {
            let at = index * 100;
            masker.hold(at, at as f64 / RATE, 100);
            assert!(
                masker.release(false).is_empty(),
                "ten seconds is a long lag"
            );
        }
        let out = masker.release(true);
        assert_eq!(out.len(), 5, "the end of the stream settles everything");
        assert_eq!(
            out.iter().map(|e| e.pts).collect::<Vec<_>>(),
            vec![0, 100, 200, 300, 400]
        );
    }

    #[test]
    fn a_span_told_after_its_audio_passed_still_masks_it() {
        let mut masker = masker(1, params(0.0, 0.0, 0.5));
        // The cue covers the second payload and is told on the fifth, which
        // is what a recognizer whose rows ride a late window does.
        let out = run(&mut masker, 10, &[(4, cue(0.1, 0.2))]);
        let second = out.iter().find(|e| e.pts == 100).expect("the payload");
        assert_eq!(values(second, 1)[50], 1.0, "the late span reached it");
    }

    #[test]
    fn a_span_told_past_the_lag_is_too_late_for_its_own_audio() {
        let mut masker = masker(1, params(0.0, 0.0, 0.2));
        // The same cue, told five payloads after the audio it describes has
        // already been written out.
        let out = run(&mut masker, 10, &[(8, cue(0.1, 0.2))]);
        let second = out.iter().find(|e| e.pts == 100).expect("the payload");
        assert_eq!(values(second, 1)[50], 0.0, "that mask was already written");
    }

    /// Six seconds of audio with one 3.6-second span in it, told on the
    /// window that CLOSED the span - which is when a recognizer tells it.
    fn a_long_span(lag: f64) -> Vec<Emitted> {
        let mut masker = masker(1, params(0.0, 0.0, lag));
        run(&mut masker, 60, &[(41, cue(0.5, 4.1))])
    }

    #[test]
    fn the_default_lag_outlasts_a_span_of_several_seconds() {
        let out = a_long_span(default_lag());
        for pts in [600, 2000, 4000] {
            let held = out.iter().find(|e| e.pts == pts).expect("the payload");
            assert_eq!(values(held, 1)[0], 1.0, "{pts} is inside the span");
        }
    }

    #[test]
    fn two_seconds_of_lag_is_too_short_for_the_same_span() {
        // What the default was, and why it is not: the row naming the span
        // arrives at 4.1 s, by which time a two-second lag has already
        // written the mask for everything up to 2.1 s as background.
        let out = a_long_span(2.0);
        let early = out.iter().find(|e| e.pts == 600).expect("the payload");
        assert_eq!(values(early, 1)[0], 0.0, "its mask was already written");
        let late = out.iter().find(|e| e.pts == 4000).expect("the payload");
        assert_eq!(values(late, 1)[0], 1.0, "only the tail of it is masked");
    }

    #[test]
    fn a_span_is_dropped_once_the_audio_past_it_has_been_written() {
        let mut masker = masker(1, params(0.0, 0.0, 0.0));
        masker.learn(&rows(&[&cue(0.0, 0.1)]));
        masker.hold(0, 0.0, 100);
        masker.release(false);
        assert_eq!(masker.spans.len(), 1, "the mask ends exactly at the span");
        masker.hold(100, 0.1, 100);
        masker.release(false);
        assert!(masker.spans.is_empty(), "the audio past it is written");
    }

    #[test]
    fn a_span_still_ahead_of_the_mask_is_kept() {
        let mut masker = masker(1, params(0.0, 0.0, 0.0));
        masker.learn(&rows(&[&cue(5.0, 6.0)]));
        masker.hold(0, 0.0, 100);
        masker.release(false);
        assert_eq!(masker.spans.len(), 1, "its audio has not arrived yet");
    }

    #[test]
    fn rows_that_are_not_cues_are_skipped_rather_than_refused() {
        let mut masker = masker(1, params(0.0, 0.0, 0.0));
        masker.learn(&rows(&[
            r#"{"shot":4}"#,
            "not json at all",
            r#"{"start_t":"half past","end_t":2}"#,
            r#"{"start_t":0.4,"end_t":0.2}"#,
            &cue(0.1, 0.2),
        ]));
        assert_eq!(masker.spans.len(), 1, "one row of five was a span");
        masker.hold(0, 0.0, 300);
        let mask = values(&masker.release(true)[0], 1);
        assert_eq!(mask[150], 1.0);
        assert_eq!(mask[250], 0.0);
    }

    #[test]
    fn no_spans_at_all_is_an_entirely_silent_mask() {
        let mut masker = masker(1, params(50.0, 50.0, 0.0));
        masker.hold(0, 0.0, 100);
        let out = masker.release(true);
        assert!(values(&out[0], 1).iter().all(|v| *v == 0.0));
    }

    #[test]
    fn params_outside_the_schema_are_refused_by_name() {
        assert!(parse_params(r#"{"grow":-1}"#).is_err());
        assert!(parse_params(r#"{"feather":100000}"#).is_err());
        assert!(parse_params(r#"{"lag":-0.5}"#).is_err());
        assert!(parse_params(r#"{"lag":120}"#).is_err());
        assert!(parse_params(r#"{"delay":2}"#).is_err());
    }

    #[test]
    fn no_params_at_all_are_the_defaults() {
        for written in ["", "{}", "  "] {
            let parsed = parse_params(written).expect("the defaults");
            assert_eq!((parsed.grow, parsed.feather, parsed.lag), (0.0, 0.0, 32.0));
        }
    }

    #[test]
    fn each_parameter_can_be_set_on_its_own() {
        let parsed = parse_params(r#"{"grow":120}"#).expect("one of them");
        assert_eq!(parsed.grow, 120.0);
        assert_eq!(parsed.lag, 32.0, "the rest keep their defaults");
    }

    #[test]
    fn a_timestamp_becomes_the_seconds_its_time_base_says() {
        assert_eq!(seconds(48_000, 1, 48_000), 1.0);
        assert_eq!(seconds(-4410, 1, 44_100), -0.1);
    }
}
