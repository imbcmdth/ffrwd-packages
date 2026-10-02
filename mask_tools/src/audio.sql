-- Composition over an audio mask, the matte one dimension down. A mask is an
-- audio stream at the track's own rate and channel count, 1.0 inside a span
-- and 0 outside; `amultiply` weighs one stream by another sample for sample,
-- and `amix` with `normalize => false` adds what comes out without halving it.
--
-- The two wasm modules are what native ffmpeg cannot spell. `spans_mask`
-- turns the cues a recognizer writes into that mask. `tone` is a sine as long
-- as the stream it is handed: a generated source has to be told a duration,
-- and a bleep would rather be told by the track.
--
-- `1 - mask` is the one piece of arithmetic ffmpeg has no filter for. `aeval`
-- writes the expression but not the channels: given one expression it hands
-- back one channel whatever arrived, and a function with a stream parameter
-- has no channel count to write instead. `volume` then `dcshift` is the same
-- arithmetic over however many channels there are - negate, then add one.
--
-- `spans_mask` reads `a` for its rate, layout and timing and never for its
-- samples, and writes the mask like it. `cues` are any rows carrying
-- `start_t` and `end_t`, narrowed with a gather when only some should mask:
-- `ARRAY(SELECT c FROM unnest(<rows>) c WHERE c.text ILIKE ANY (...))`. Each
-- cue reaches the mask by its time, however late the recognizer writes it,
-- because the host holds the audio until the recognizer has gone past it:
-- for a transcriber, the 30 s window the word was heard in. `grow` pads every
-- span by that many milliseconds, `feather` ramps its edges over that many,
-- and the cues are fetched that far ahead so a ramp starts on time.
CREATE FUNCTION spans_mask(a audio_stream, cues STRUCT(start_t number, end_t number)[],
                           grow number DEFAULT 0, feather number DEFAULT 0)
RETURNS audio_stream
  AS 'target/wasm32-wasip2/release/spans_mask.wasm', 'spans_mask' LANGUAGE wasm;

CREATE FUNCTION tone(a audio_stream, frequency number DEFAULT 1000, level number DEFAULT 0.3)
RETURNS audio_stream
  AS 'target/wasm32-wasip2/release/tone.wasm', 'tone' LANGUAGE wasm;

-- `other` where the mask is 1, `a` where it is 0, and the crossfade the
-- feathered edge asks for in between.
--
-- Spelled `a + mask * (other - a)` rather than the plainer
-- `a * (1 - mask) + other * mask`, which names the mask twice. A mask is
-- usually what a recognizer's module just produced, and a stream named twice
-- is that module run twice - the whole detector again for the second copy.
-- `a` is named twice instead, and a track is only read twice.
CREATE FUNCTION replace_where(a audio_stream, mask audio_stream, other audio_stream)
RETURNS audio_stream AS $$
  SELECT amix(a, amultiply(mask, amix(other, volume(a, -1), normalize => false)),
              normalize => false)
$$ LANGUAGE sql;

-- `a` where the mask is 0, silence where it is 1. A mask cut at the track's
-- own rate is exact both ways: the kept audio is the track's own samples and
-- the muted ones are zero.
CREATE FUNCTION mute_where(a audio_stream, mask audio_stream)
RETURNS audio_stream AS $$
  SELECT amultiply(a, dcshift(volume(mask, -1), 1))
$$ LANGUAGE sql;

-- The censor's bleep: a sine the length of the track, laid in where the mask
-- marks and nowhere else.
CREATE FUNCTION bleep_where(a audio_stream, mask audio_stream,
                            frequency number DEFAULT 1000, level number DEFAULT 0.3)
RETURNS audio_stream AS $$
  SELECT replace_where(a, mask, tone(a, frequency, level))
$$ LANGUAGE sql;
