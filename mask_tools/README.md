# ffrwd/mask_tools

Composition over a matte, pure SQL over native ffmpeg. A matte is a video stream read as a per-pixel weight: white keeps the
overlay, black keeps the base, gray blends. Anything that produces one
works here - a segmentation model, a rasterized box list, a depth map,
a hand-drawn ramp.

## Exports

- `masked(v, matte, filtered)` - the one `maskedmerge` - reused by the rest. `v` where the matte is black `filtered` where it is white.
- `blur_where(v, matte, sigma DEFAULT 12)` - blurs.
- `mosaic_where(v, matte, size DEFAULT 16)` - pixelates.
- `spotlight(v, matte, dim DEFAULT 0.6)` - darkens everything but the matte.
- `cutout(v, matte, background)` - the matte as alpha, over a new background.
