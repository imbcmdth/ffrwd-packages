# ffrwd/mask_tools

Composition over a matte, pure SQL over native ffmpeg. A matte is a
video stream read as a per-pixel weight: white keeps the overlay,
black keeps the base, gray blends. Anything that produces one works
here - a segmentation model, a rasterized box list, a depth map, a
hand-drawn ramp.

The same shape one dimension down for audio: a mask is an audio
stream read as a per-sample weight, 1 keeps the other track, 0 keeps
the base, a ramp crossfades. Anything that produces spans makes one -
a voice detector, a transcriber, a hand-written cue list.

## Exports

Video:

- `masked(v, matte, filtered)` - the one merge - reused by the rest. `v` where the matte is black, `filtered` where it is white. The matte rides in as an alpha channel, so white replaces the picture outright and a `feather` edge ramps between the two.
- `blur_where(v, matte, sigma DEFAULT 12)` - blurs.
- `mosaic_where(v, matte, size DEFAULT 16)` - pixelates.
- `spotlight(v, matte, dim DEFAULT 0.6)` - darkens everything but the matte.
- `cutout(v, matte, background)` - the matte as alpha, over a new background.

Audio:

- `spans_mask(a, cues, grow DEFAULT 0, feather DEFAULT 0, lag DEFAULT 32)` - the rows rasterized into a mask, the twin of a box list's matte: 1 inside each cue's span, 0 outside. `grow` pads each span by that many milliseconds; `feather` ramps the edges over that many. Overlapping spans union. A wasm module, reading the rows that ride its input.
- `fan(mask, channels)` - the mask in that many identical channels, at unity: what a mask cut from a mono recognizer needs before it meets a stereo or 5.1 track.
- `replace_where(a, mask, other)` - the one merge - reused by the rest. `a` where the mask is 0, `other` where it is 1: `a + mask * (other - a)`, over `amultiply` and `amix`.
- `mute_where(a, mask)` - silence.
- `bleep_where(a, mask, frequency DEFAULT 1000, level DEFAULT 0.3)` - a tone.
- `tone(a, frequency DEFAULT 1000, level DEFAULT 0.3)` - a sine the length of `a`, at its rate and channels, `a`'s samples never read. A wasm module, because a generated source has no length to inherit.

## Two things about the mask

**It runs late.** A recognizer's row arrives with the window the span
closed on, after the audio it describes has passed through: a voice
detector's about a second after, a transcriber's at the end of its
30-second window, and in both cases only once the span has ended. So
the mask holds audio back for `lag` seconds and writes each stretch at
its original time once that much later audio has been seen, or at the
end of the stream. The merge downstream waits, as the video
merge waits on a slow detector. `lag` has to exceed the longest span, not the
recognizer's delay: a span still open when the held audio is written
is written unmasked. The default holds 32 seconds, 12 MB of 48 kHz
stereo; raise it for long uninterrupted speech.

**It carries the recognizer's layout.** A detector conforms its input
to its own format - vad's is 16 kHz mono - and a module's output has
the format its instance was opened with, so the mask cut from it is
16 kHz mono. Left to ffmpeg, a mono mask meeting a stereo track is
upmixed at 0.707, 3 dB down outside the spans, and meeting a 5.1
track lands in the centre channel alone, five channels wiped. So
`fan` the mask to the track's own count first. The rate difference
resamples, and a resampled 1 is 1 to within a part in a hundred
thousand: `mute_where` on such a mask is digital silence inside the
spans and the original to within -100 dB outside, and
`bleep_where` the reverse. A mask cut at the track's own rate is
exact both ways.

```sql
SELECT ffrwd.mask_tools.mute_where(
         f.audio[1],
         ffrwd.mask_tools.fan(ffrwd.mask_tools.spans_mask(ffrwd.vad.speech(f.audio[1]), 100, 50), 2)),
       f.video
FROM input(:'source') f
```

The struct a recognizer returns spreads into `a` and `cues` the way a
detector's spreads into `boxes_mask`. To mask some spans and not
others, narrow the rows first with the gather spelling:
`ARRAY(SELECT c FROM unnest(ffrwd.whisper.transcribe(a).words) c WHERE c.text ILIKE '%damn%')` (`ILIKE` needs ffrwd 0.17).

## Building

The two audio modules build against the wit from the installed
`ffrwd/wasm` package:

```
ffrwd install -g ffrwd/wasm
cargo build --target wasm32-wasip2 --release
```
