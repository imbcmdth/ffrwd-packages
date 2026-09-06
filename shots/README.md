# ffrwd/shots

Hard-cut detection, hosted in wasm. `simple_detector(v)` passes the
frame through untouched and hands each one a `{"shot": n}` row - the
shot counter steps whenever a frame's luma grid differs enough from
the one before it.

## License

This package is **MIT**.

## Export

- `simple_detector(v, threshold DEFAULT 12)` returns `STRUCT(v video_stream,
  cuts STRUCT(shot number)[])` - the stream, and the per-frame shot rows
  beside it.

## Recipes

- `cut-points` - the shot rows themselves, written to a rows file.

```
ffrwd ffrwd.shots.cut-points -v source=film.mp4 -v dest=cuts.ndjson
```
