# ffrwd/depth

Monocular depth as a matte. `depth(v)` returns the
scene's depth as a grayscale video stream - near bright, far dark. Ready for everything that reads a matte!

There is no invert option. ffmpeg's own `negate` filter is the
inversion: `negate(ffrwd.depth.depth(v))` is far-bright.

## License

This package is **MIT**. The model weights are Depth Anything V2
(small), released under **Apache-2.0**, and using them stays under that
license; nothing here changes it.

The weights are not in the archive: the manifest pins them - exact
repo, revision, file and sha256 - and `ffrwd install` fetches and
verifies them. The graph is the fp32 ONNX export, about 95 MB.

## Export

- `depth(v)` returns the depth matte as a `video_stream`, same
  geometry and pixel format as `v`.

## Recipes

- `bokeh` - blur the far field, keep the near sharp.
- `depth-map` - the matte itself, written to a file.
- `near-spotlight` - dim everything but the near field.

```
ffrwd ffrwd.depth.bokeh -v source=street.mp4 -v dest=bokeh.mp4
```
