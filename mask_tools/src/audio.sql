-- Composition over an audio mask, the matte one dimension down. A mask is an
-- audio stream at the track's own rate and channel count, 1.0 inside a span
-- and 0 outside; `amultiply` weighs one stream by another sample for sample,
-- and `amix` with `normalize => false` adds what comes out without halving it.
--
-- The two wasm modules are what native ffmpeg cannot spell. `spans_mask`
-- turns the cues a recognizer writes into that mask - and holds the audio
-- back while it waits for them, since a row always arrives after the audio it
-- describes. `tone` is a sine as long as the stream it is handed: a generated
-- source has to be told a duration, and a bleep would rather be told by the
-- track.
--
-- `1 - mask` is the one piece of arithmetic ffmpeg has no filter for. `aeval`
-- writes the expression but not the channels: given one expression it hands
-- back one channel whatever arrived, and a function with a stream parameter
-- has no channel count to write instead. `volume` then `dcshift` is the same
-- arithmetic over however many channels there are - negate, then add one.
--
-- A mask belongs to the audio it was cut from, and these three want both at
-- the same rate and the same channel count. `spans_mask` takes its input's,
-- and cannot be asked for anything else: `out-frame` carries bytes and a
-- timestamp, `window-meta` names only what the module ACCEPTS, and an audio
-- instance's `audio-format` is settled at `init` and "fixed for the life of
-- the instance" - so there is no field a different channel count could reach
-- the host through, and the bytes are read back in the format the instance
-- opened with. A recognizer conforms its input to its own format, so a mask
-- cut from one is that recognizer's shape: for a VAD, 16 kHz mono.
--
-- `fan` is what a merge wants in between. Hand a merge a mono mask and a
-- track of more channels and ffmpeg rematrixes it on the way in, by a rule
-- that belongs to the two layouts rather than to the mask: mono into stereo
-- is 1/sqrt(2) on both, 3 dB off everything the mask touches, and mono into
-- 5.1 is the whole mask into the centre and SILENCE in the other five. No
-- gain undoes both, so the count is named instead.
CREATE FUNCTION spans_mask(a audio_stream, cues cue[],
                           grow number DEFAULT 0, feather number DEFAULT 0,
                           lag number DEFAULT 32)
RETURNS audio_stream
  AS 'target/wasm32-wasip2/release/spans_mask.wasm', 'spans_mask' LANGUAGE wasm;

-- The same mask in `channels` channels, one copy of channel 0 per channel,
-- at unity - which is what a merge needs before a mask of one shape meets a
-- track of another. Written with `pan` and not `channelmap`: `channelmap`
-- hands its outputs on as several names for ONE buffer when it is asked for
-- the same input channel twice, and the arithmetic the merges do in place
-- then reads its own writes - measured, a mask that mutes nothing.
--
-- Naming a count is not naming a layout, so ffmpeg's own `<n>c` spelling is
-- what goes in. The per-channel half `pan` wants has no spelling that says
-- "and the rest the same", so the counts are written out one line each, mono
-- through 7.1; past eight the arguments come out empty and ffmpeg refuses
-- them when the run starts, since a count is not a stream and nothing here
-- can reject it earlier.
CREATE FUNCTION fan(m audio_stream, channels number)
RETURNS audio_stream AS $$
  SELECT pan(m, args => channels::text || 'c' || CASE channels
                WHEN 1 THEN '|c0=c0'
                WHEN 2 THEN '|c0=c0|c1=c0'
                WHEN 3 THEN '|c0=c0|c1=c0|c2=c0'
                WHEN 4 THEN '|c0=c0|c1=c0|c2=c0|c3=c0'
                WHEN 5 THEN '|c0=c0|c1=c0|c2=c0|c3=c0|c4=c0'
                WHEN 6 THEN '|c0=c0|c1=c0|c2=c0|c3=c0|c4=c0|c5=c0'
                WHEN 7 THEN '|c0=c0|c1=c0|c2=c0|c3=c0|c4=c0|c5=c0|c6=c0'
                WHEN 8 THEN '|c0=c0|c1=c0|c2=c0|c3=c0|c4=c0|c5=c0|c6=c0|c7=c0'
              END)
$$ LANGUAGE sql;

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

-- `a` where the mask is 0, silence where it is 1.
--
-- `fan` settles a mask's channels, and a rate nothing here can settle: a
-- merge cannot name the track's, so a mask cut at another one is converted by
-- whatever ffmpeg inserts last, after all of this. A resampler carries a run
-- of ZEROS across exactly - zero times any coefficient is zero - and a run of
-- ones only to about a part in 1e5, so which of the two ends up exact is
-- decided by which one the mask is carrying when it crosses. Here the
-- negate-and-add-one runs first, at the mask's own rate, so it is the MUTED
-- samples that cross as zeros: the silence is digital, and the kept audio
-- reads 0.99999 of itself. `replace_where` hands its mask over as it is and
-- comes out the other way round - what it keeps is untouched, what it
-- replaces keeps a part in 1e5 of the original underneath. Neither costs
-- anything at all when the mask was cut at the track's own rate.
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
