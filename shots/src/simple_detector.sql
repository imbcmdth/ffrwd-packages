-- Hard-cut detection over a 32x32 luma grid: each frame's grid is compared
-- with the previous frame's, and a mean absolute difference above
-- `threshold` steps the shot counter. The first frame is shot 0. Frames pass
-- through untouched; the row a frame carries away, `{"shot": n}`, is what a
-- downstream module reads to know a cut happened.
CREATE FUNCTION simple_detector(v video_stream, threshold number DEFAULT 12)
RETURNS STRUCT(v video_stream, cuts STRUCT(shot number)[])
  AS 'target/wasm32-wasip2/release/simple_detector.wasm', 'simple_detector'
  LANGUAGE wasm;
