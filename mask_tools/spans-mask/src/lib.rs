//! Cues to a mask: the spans a recognizer reports become an audio stream at
//! the track's own rate and channel count - 1.0 inside a span, 0 outside,
//! and a linear ramp between where `feather` asks for one. `grow` pads every
//! span outward in milliseconds; `feather` softens each edge over that many,
//! falling from the grown span's edge to nothing.
//!
//! The samples themselves are never listened to - only how many arrived and
//! when - so the mask can be multiplied back against the track by whatever
//! consumes a mask beside it. A row that is not a cue is skipped rather than
//! refused.
//!
//! # When a cue arrives
//!
//! The cues are paired with the audio by their time: a tick is handed every
//! cue that starts inside it, and the host holds the tick until the
//! recognizer has gone past it, so a cue is there by the tick it starts on
//! however late the recognizer wrote it. `grow` and `feather` reach before a
//! cue's start, so the cues input reaches that far ahead of each tick too,
//! and a cue arrives on the tick its ramp begins on. Each cue is kept as
//! state until the mask has been written past its end, so a span longer than
//! a tick masks every tick it covers, and a node on another worker is handed
//! the cues of the ticks it did not see.
//!
//! The mask leaves at the pts the audio arrived at, a run for a run: the node
//! is one-to-one, and pure, since what it keeps is only what the cues said.

use ffrwd_node::{Bound, Init, Input, Node, Out, Output, Rational, Result, Shape, StateRow, Tick};
use serde::Deserialize;

/// Samples one tick carries. A tenth of a second at 48 kHz: coarse enough
/// that a stream's worth of ticks is cheap.
const WINDOW: u32 = 4096;

/// The width of one f32 value, which is what the audio arrives and leaves as.
const SAMPLE_BYTES: usize = 4;

/// Milliseconds in a second, which is the unit `grow` and `feather` are in.
const MS: f64 = 1000.0;

const PARAMS_SCHEMA: &str = r#"{"type":"object","properties":{"grow":{"type":"number","minimum":0,"maximum":60000,"default":0},"feather":{"type":"number","minimum":0,"maximum":60000,"default":0}},"additionalProperties":false}"#;

/// What a cue has to carry to be masked: the seconds it runs between. Any
/// other field passes.
const CUES_SCHEMA: &str = r#"{"type":"object","properties":{"start_t":{"type":"number"},"end_t":{"type":"number"}},"required":["start_t","end_t"]}"#;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
struct Params {
    grow: f64,
    feather: f64,
}

impl Params {
    /// Seconds a span's mask reaches past either edge.
    fn reach(&self) -> f64 {
        (self.grow + self.feather) / MS
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
#[derive(Clone, Copy, Debug, PartialEq)]
struct Span {
    start_t: f64,
    end_t: f64,
}

/// The spans told so far that the mask has not yet been written past.
struct Masker {
    rate: f64,
    channels: usize,
    params: Params,
    spans: Vec<Span>,
}

impl Masker {
    fn new(rate: f64, channels: usize, params: Params) -> Masker {
        Masker {
            rate,
            channels,
            params,
            spans: Vec::new(),
        }
    }

    /// One row. A row that is not a cue, and a span that ends before it
    /// starts, are skipped; a span already told is not told twice.
    fn learn(&mut self, row: &str) {
        let Ok(cue) = serde_json::from_str::<Cue>(row) else {
            return;
        };
        let span = Span {
            start_t: cue.start_t,
            end_t: cue.end_t,
        };
        if !span.start_t.is_finite() || !span.end_t.is_finite() || span.end_t < span.start_t {
            return;
        }
        if !self.spans.contains(&span) {
            self.spans.push(span);
        }
    }

    /// Forgets the spans that can say nothing about the audio from `at` on.
    fn forget(&mut self, at: f64) {
        let reach = self.params.reach();
        self.spans.retain(|span| span.end_t + reach >= at);
    }

    /// `samples` of mask from `start_t` on, the same value in every channel.
    fn paint(&self, start_t: f64, samples: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(samples * self.channels * SAMPLE_BYTES);
        for index in 0..samples {
            let at = start_t + index as f64 / self.rate;
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

struct SpansMask {
    a: u32,
    time_base: Rational,
    /// Whether a run's duration counts its samples, which is when its bytes
    /// need not be fetched to know how long it is.
    counts_samples: bool,
    /// How far ahead of a tick the shape asked for cues, in seconds.
    declared: f64,
    masker: Masker,
}

impl Node for SpansMask {
    const NAME: &'static str = "spans_mask";
    const VERSION: &'static str = "0.2.0";
    const PARAMS_SCHEMA: &'static str = PARAMS_SCHEMA;
    type Params = Params;

    fn shape(params: &Params, _: &Bound) -> Result<Shape> {
        Ok(Shape::new()
            .input(
                Input::audio("a")
                    .clock()
                    .window(WINDOW, WINDOW)
                    .sample_formats(&["f32"]),
            )
            .input(
                Input::rows("cues")
                    .interval()
                    .ahead(params.reach())
                    .state()
                    .schema_json(CUES_SCHEMA),
            )
            .output(Output::like("a"))
            .pure()
            .one_to_one())
    }

    fn init(params: Params, init: &Init) -> Result<SpansMask> {
        let a = init.stream("a")?;
        let audio = a
            .audio_format()
            .ok_or("spans_mask masks samples, and `a` is not audio")?;
        Ok(SpansMask {
            a: a.id,
            time_base: a.info.time_base,
            counts_samples: a.info.time_base == Rational::new(1, audio.sample_rate as i32),
            declared: params.reach(),
            masker: Masker::new(
                f64::from(audio.sample_rate),
                audio.channels as usize,
                params,
            ),
        })
    }

    fn set_params(&mut self, params: Params) -> Result<()> {
        // A wider reach would want cues sooner than the shape asks for them.
        if params.reach() > self.declared {
            return Err(format!(
                "spans_mask: grow and feather reach {} ms before a cue, and the node opened \
                 reaching {} ms; they can narrow while it runs but not widen",
                params.reach() * MS,
                self.declared * MS
            )
            .into());
        }
        self.masker.params = params;
        Ok(())
    }

    fn fold(&mut self, row: StateRow) -> Result<()> {
        self.masker.learn(row.json);
        Ok(())
    }

    fn process(&mut self, tick: &Tick, out: &mut Out) -> Result<()> {
        let width = self.masker.channels * SAMPLE_BYTES;
        for frame in tick.frames(self.a) {
            let samples = match frame.duration {
                Some(duration) if self.counts_samples => duration as usize,
                _ => tick.fetch(self.a, frame.index).len() / width,
            };
            let start_t = self.time_base.seconds(frame.pts);
            self.masker.forget(start_t);
            let mask = self.masker.paint(start_t, samples);
            out.frame("a", frame.pts, frame.duration, mask)?;
        }
        Ok(())
    }
}

ffrwd_node::export!(SpansMask);

#[cfg(test)]
mod tests {
    use super::*;
    use ffrwd_node::mock::Harness;
    use ffrwd_node::{BoundStream, Pairing, Payload};

    const RATE: f64 = 1000.0;

    fn params(grow: f64, feather: f64) -> Params {
        Params { grow, feather }
    }

    fn masker(channels: usize, params: Params) -> Masker {
        Masker::new(RATE, channels, params)
    }

    fn told(masker: &mut Masker, rows: &[&str]) {
        for row in rows {
            masker.learn(row);
        }
    }

    /// One cue row, in seconds.
    fn cue(start_t: f64, end_t: f64) -> String {
        format!(r#"{{"text":"speech","start_t":{start_t},"end_t":{end_t}}}"#)
    }

    /// A painted mask's values, one per sample of one channel.
    fn values(painted: &[u8], channels: usize) -> Vec<f32> {
        let (whole, _) = painted.as_chunks::<4>();
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

    #[test]
    fn a_span_masks_its_own_seconds_and_nothing_else() {
        let mut masker = masker(1, params(0.0, 0.0));
        told(&mut masker, &[&cue(0.2, 0.4)]);
        let mask = values(&masker.paint(0.0, 1000), 1);
        assert_eq!(mask[100], 0.0, "before the span");
        assert_eq!(mask[200], 1.0, "the first sample of the span");
        assert_eq!(mask[300], 1.0, "inside it");
        assert_eq!(mask[400], 1.0, "the last sample of the span");
        assert_eq!(mask[401], 0.0, "one past it is background");
    }

    #[test]
    fn grow_pads_the_span_outward() {
        let mut masker = masker(1, params(50.0, 0.0));
        told(&mut masker, &[&cue(0.2, 0.4)]);
        let mask = values(&masker.paint(0.0, 1000), 1);
        assert_eq!(mask[152], 1.0, "fifty milliseconds before the span");
        assert_eq!(mask[448], 1.0, "and fifty after it");
        assert_eq!(mask[148], 0.0, "past the growth is background");
        assert_eq!(mask[452], 0.0);
    }

    #[test]
    fn feather_ramps_linearly_outside_the_grown_span() {
        let mut masker = masker(1, params(50.0, 100.0));
        told(&mut masker, &[&cue(0.3, 0.5)]);
        let mask = values(&masker.paint(0.0, 1000), 1);
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
        let mut masker = masker(1, params(0.0, 0.0));
        told(&mut masker, &[&cue(0.1, 0.3), &cue(0.2, 0.5)]);
        let mask = values(&masker.paint(0.0, 1000), 1);
        assert_eq!(mask[250], 1.0, "the overlap is full, not doubled");
        assert_eq!(mask[150], 1.0);
        assert_eq!(mask[450], 1.0);
        assert_eq!(mask[50], 0.0);
        assert_eq!(mask[550], 0.0);
    }

    #[test]
    fn the_mask_carries_the_same_value_in_every_channel() {
        let mut masker = masker(2, params(0.0, 0.0));
        told(&mut masker, &[&cue(0.0, 1.0)]);
        let painted = masker.paint(0.0, 10);
        assert_eq!(painted.len(), 10 * 2 * 4, "ten stereo samples");
        let (whole, _) = painted.as_chunks::<4>();
        assert!(whole.iter().copied().all(|b| f32::from_le_bytes(b) == 1.0));
    }

    #[test]
    fn a_span_is_dropped_once_the_audio_past_it_has_been_written() {
        let mut masker = masker(1, params(0.0, 0.0));
        told(&mut masker, &[&cue(0.0, 0.1)]);
        masker.forget(0.1);
        assert_eq!(masker.spans.len(), 1, "the mask ends exactly at the span");
        masker.forget(0.2);
        assert!(masker.spans.is_empty(), "the audio past it is written");
    }

    #[test]
    fn a_span_still_ahead_of_the_mask_is_kept() {
        let mut masker = masker(1, params(0.0, 0.0));
        told(&mut masker, &[&cue(5.0, 6.0)]);
        masker.forget(0.1);
        assert_eq!(masker.spans.len(), 1, "its audio has not arrived yet");
    }

    #[test]
    fn a_ramp_keeps_its_span_until_the_mask_is_past_the_ramp_too() {
        let mut masker = masker(1, params(50.0, 100.0));
        told(&mut masker, &[&cue(0.0, 0.1)]);
        masker.forget(0.24);
        assert_eq!(masker.spans.len(), 1, "0.25 s is the end of its ramp");
        masker.forget(0.26);
        assert!(masker.spans.is_empty());
    }

    #[test]
    fn rows_that_are_not_cues_are_skipped_rather_than_refused() {
        let mut masker = masker(1, params(0.0, 0.0));
        told(
            &mut masker,
            &[
                r#"{"shot":4}"#,
                "not json at all",
                r#"{"start_t":"half past","end_t":2}"#,
                r#"{"start_t":0.4,"end_t":0.2}"#,
                &cue(0.1, 0.2),
            ],
        );
        assert_eq!(masker.spans.len(), 1, "one row of five was a span");
        let mask = values(&masker.paint(0.0, 300), 1);
        assert_eq!(mask[150], 1.0);
        assert_eq!(mask[250], 0.0);
    }

    #[test]
    fn no_spans_at_all_is_an_entirely_silent_mask() {
        let masker = masker(1, params(50.0, 50.0));
        assert!(values(&masker.paint(0.0, 100), 1).iter().all(|v| *v == 0.0));
    }

    #[test]
    fn params_outside_the_schema_are_refused_by_name() {
        assert!(read(r#"{"grow":-1}"#).is_err());
        assert!(read(r#"{"feather":100000}"#).is_err());
        assert!(read(r#"{"delay":2}"#).is_err());
        assert!(
            read(r#"{"lag":32}"#).is_err(),
            "the host holds the audio for the cues now"
        );
    }

    #[test]
    fn no_params_at_all_are_the_defaults() {
        for written in ["", "{}", "  "] {
            let parsed = read(written).expect("the defaults");
            assert_eq!((parsed.grow, parsed.feather), (0.0, 0.0));
        }
    }

    #[test]
    fn each_parameter_can_be_set_on_its_own() {
        let parsed = read(r#"{"grow":120}"#).expect("one of them");
        assert_eq!(parsed.grow, 120.0);
        assert_eq!(parsed.feather, 0.0, "the rest keep their defaults");
    }

    // ------------------------------------------------------------ the node

    fn bound() -> Vec<BoundStream> {
        vec![
            BoundStream::audio("a", 0, 1000, 2, "f32"),
            BoundStream::rows("cues", 1, Rational::new(1, 16_000)),
        ]
    }

    fn mask_of(emitted: &ffrwd_node::Emitted) -> (i64, Option<i64>, Vec<f32>) {
        let [Payload::Frame {
            pts,
            duration,
            data,
        }] = &emitted.on("a")[..]
        else {
            panic!("one run out per run in: {emitted:?}")
        };
        (*pts, *duration, values(data, 2))
    }

    #[test]
    fn the_cues_reach_as_far_ahead_as_the_ramp_and_the_mask_is_like_its_audio() {
        let node =
            Harness::<SpansMask>::new(r#"{"grow":100,"feather":20}"#, bound()).expect("opens");
        let shape = node.shape();
        let a = shape.find_input("a").expect("a");
        assert_eq!((a.window, a.stride), (WINDOW, WINDOW));
        let cues = shape.find_input("cues").expect("cues");
        let Pairing::Interval(interval) = &cues.pairing else {
            panic!("paired by time: {:?}", cues.pairing)
        };
        assert!((interval.ahead - 0.12).abs() < 1e-12);
        assert_eq!(interval.latency, None, "as long as the recognizer takes");
        assert_eq!(cues.rows, ffrwd_node::RowsUse::State);
        let like = shape.outputs[0].like.as_ref().expect("follows `a`");
        assert_eq!(like.port.as_deref(), Some("a"));
        assert!(shape.pure && shape.one_to_one);
    }

    #[test]
    fn a_cue_masks_the_ticks_it_covers_and_its_samples_stay_unread() {
        let mut node = Harness::<SpansMask>::new("", bound()).expect("opens");
        // The run's bytes are empty: a node that fetched them to count them
        // would paint nothing.
        let first = node
            .tick(0)
            .frame_with(0, 0, Some(1000), &[], Vec::new())
            .row(
                1,
                8_000,
                &serde_json::json!({"text":"hi","start_t":0.5,"end_t":1.5}),
            );
        let (pts, duration, mask) = mask_of(&node.process(&first).expect("processes"));
        assert_eq!((pts, duration, mask.len()), (0, Some(1000), 1000));
        assert_eq!((mask[499], mask[500], mask[999]), (0.0, 1.0, 1.0));
        let second = node
            .tick(1000)
            .frame_with(0, 1000, Some(1000), &[], Vec::new());
        let (_, _, mask) = mask_of(&node.process(&second).expect("processes"));
        assert_eq!((mask[500], mask[501]), (1.0, 0.0), "kept until it ends");
    }

    #[test]
    fn a_worker_that_did_not_see_a_cue_arrive_is_handed_it() {
        let mut node = Harness::<SpansMask>::new("", bound()).expect("opens");
        let tick = node
            .tick(2000)
            .frame_with(0, 2000, Some(1000), &[], Vec::new())
            .earlier(1, 8_000, &[&cue(0.5, 2.25)]);
        let (_, _, mask) = mask_of(&node.process(&tick).expect("processes"));
        assert_eq!((mask[249], mask[250], mask[251]), (1.0, 1.0, 0.0));
    }

    #[test]
    fn grow_and_feather_narrow_live_and_do_not_widen() {
        let mut node =
            Harness::<SpansMask>::new(r#"{"grow":100,"feather":20}"#, bound()).expect("opens");
        node.set_params(r#"{"grow":50,"feather":20}"#)
            .expect("a narrower reach");
        node.set_params(r#"{"grow":100,"feather":20}"#)
            .expect("and back to the one it opened with");
        let refused = node.set_params(r#"{"grow":200}"#).expect_err("a wider one");
        assert!(refused.contains("not widen"), "{refused}");
    }
}
