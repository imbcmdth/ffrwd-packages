# ffrwd/mask_tools

Composition over a matte, pure SQL over native ffmpeg. No model, no
wasm: any grayscale matte beside any stream, and the filtering it
selects happens in the encoder's own graph.

A matte is a video stream read as a per-pixel weight: white keeps the
overlay, black keeps the base, gray blends. Anything that produces one
works here - a segmentation model, a rasterized box list, a depth map,
a hand-drawn ramp.

## Exports

- `masked(v, matte, filtered)` - the one `maskedmerge` spelling,
  reused by the rest: the picture where the matte is black, the
  filtered copy where it is white.
- `blur_where(v, matte, sigma DEFAULT 12)`
- `mosaic_where(v, matte, size DEFAULT 16)`
- `spotlight(v, matte, dim DEFAULT 0.6)` - darken everything but the
  matte.
- `cutout(v, matte, background)` - the matte as alpha, over a new
  background.

## Example

Blur wherever a matte is white - here one from
`ffrwd.yolo26.segment_mask`, but any grayscale matte does:

```sql
COPY (
  SELECT ffrwd.mask_tools.blur_where(v, ffrwd.yolo26.segment_mask(v, 'person'), 12),
         f.audio
  FROM input('street.mp4') f, unnest(f.video) v
  WHERE v.index = 1
) TO 'blurred.mp4' WITH (video_codec 'libx264', crf 20)
```
