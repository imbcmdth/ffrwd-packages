# ffrwd/depth

Monocular depth as a matte, hosted in wasm. `depth(v)` returns the
scene's depth as a grayscale video stream at the frame's own geometry -
near bright, far dark - ready for everything that reads a matte beside
the picture: `ffrwd/mask_tools`' compositions, `maskedmerge`, an alpha
channel.

There is no invert option. ffmpeg's own `negate` filter is the
inversion, and spelling it in SQL is the point:
`negate(ffrwd.depth.depth(v))` is far-bright.

## License

This package is **MIT**. The model weights are Depth Anything V2
(small), released under **Apache-2.0**, and using them stays under that
license; nothing here changes it.

The weights are not in the archive: the manifest pins them - exact
repo, revision, file and sha256 - and `ffrwd install` fetches and
verifies them. The graph is the fp32 ONNX export, about 95 MB, run
through `wasi:nn` on the machine's own ONNX Runtime. It takes the
frame letterboxed into a 518x518 square, normalized the way the model
was trained, and its map is min-max normalized over the frame's own
range and sampled back out to the frame's own size.

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
