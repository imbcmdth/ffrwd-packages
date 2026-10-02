# ffrwd/shots

Hard-cut detection, hosted in wasm. `simple_detector(v)` writes a
`{"start_t": s, "shot": n}` row for every frame - the shot counter
steps whenever a frame's luma grid differs enough from the one before
it, and `start_t` is when the frame's shot began.

Requires ffrwd 0.29, whose `ffrwd/wasm` is 0.19.1.

## License

This package is **MIT**.

## Export

- `simple_detector(v, threshold DEFAULT 12)` returns `STRUCT(start_t
  number, shot number)[]`, the rows alone. A reader takes the picture
  from the source, `reader(v, ffrwd.shots.simple_detector(v))`, and
  `ffrwd.merge_spans(ffrwd.shots.simple_detector(v), max_span => 60)`
  writes a row a shot with its `start_t` and `end_t`, a shot longer
  than 60 seconds in pieces. A shot one frame long ends a frame after
  it starts.

## Recipes

- `cut-points` - the shot rows themselves, written to a rows file.

```
ffrwd ffrwd.shots.cut-points -v source=film.mp4 -v dest=cuts.ndjson
```
