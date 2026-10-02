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

Requires ffrwd 0.29, whose `ffrwd/wasm` is 0.19.1.

## Exports

Video:

- `masked(v, matte, filtered)` - the one merge - reused by the rest. `v` where the matte is black, `filtered` where it is white. The matte rides in as an alpha channel, so white replaces the picture outright and a `feather` edge ramps between the two.
- `blur_where(v, matte, sigma DEFAULT 12)` - blurs.
- `mosaic_where(v, matte, size DEFAULT 16)` - pixelates.
- `spotlight(v, matte, dim DEFAULT 0.6)` - darkens everything but the matte.
- `cutout(v, matte, background)` - the matte as alpha, over a new background.

Audio:

- `spans_mask(a, cues, grow DEFAULT 0, feather DEFAULT 0)` - the rows rasterized into a mask, the twin of a box list's matte: 1 inside each cue's span, 0 outside, at `a`'s rate and channel count. `cues` are any rows carrying `start_t` and `end_t`. `grow` pads each span by that many milliseconds; `feather` ramps the edges over that many. Overlapping spans union. A wasm module, `a`'s samples never read.
- `replace_where(a, mask, other)` - the one merge - reused by the rest. `a` where the mask is 0, `other` where it is 1: `a + mask * (other - a)`, over `amultiply` and `amix`.
- `mute_where(a, mask)` - silence.
- `bleep_where(a, mask, frequency DEFAULT 1000, level DEFAULT 0.3)` - a tone.
- `tone(a, frequency DEFAULT 1000, level DEFAULT 0.3)` - a sine the length of `a`, at its rate and channels in f32, `a`'s samples never read. A wasm module, because a generated source has no length to inherit.

## Where the cues come from

A mask is cut from the rows a recognizer writes. Each cue reaches the
mask by its time, however late the recognizer writes it: the host holds
the audio until the recognizer has gone past it, a transcriber's 30 s
window for words, and the merge downstream waits with it as the video
merge waits on a slow detector. `grow` and `feather` reach before a
cue's start, and the cues are fetched that far ahead, so the ramp starts
on time.

The mask is the track's own rate and layout, so `mute_where` is digital
silence inside the spans and the track's own samples everywhere else,
and `bleep_where` the same the other way round.

To mask some cues and not others, narrow the rows first with a gather:

```sql
SELECT ffrwd.mask_tools.mute_where(a,
         ffrwd.mask_tools.spans_mask(a,
           ARRAY(SELECT c FROM unnest(
                   ffrwd.whisper.transcribe_words(a, speech => ffrwd.vad.speech(a), strip => true)) c
                 WHERE c.text ILIKE ANY (ARRAY['damn', 'hell'])),
           grow => 100, feather => 20)),
       f.video[1]
FROM input(:'source') f, unnest(f.audio) a
WHERE a.index = 1
```

## Building

Both modules are nodes written with `ffrwd-node`, which carries its own
world.

```
cargo build --target wasm32-wasip2 --release
```
