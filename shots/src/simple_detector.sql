-- Hard-cut detection over a 32x32 luma grid: each frame's grid is compared
-- with the previous frame's, and a mean absolute difference above
-- `threshold` steps the shot counter. The first frame is shot 0. Rows alone
-- leave it, one a frame: `{"start_t": s, "shot": n}`, `start_t` being when
-- that frame's shot began. A reader takes the picture from the source,
-- `reader(v, ffrwd.shots.simple_detector(v))`, and `ffrwd.merge_spans` turns
-- the rows into one row a shot, each as long as its frames.
CREATE FUNCTION simple_detector(v video_stream, threshold number DEFAULT 12)
RETURNS STRUCT(start_t number, shot number)[]
  AS 'target/wasm32-wasip2/release/simple_detector.wasm', 'simple_detector'
  LANGUAGE wasm;
