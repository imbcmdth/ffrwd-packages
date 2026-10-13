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

-- `simple_detector` with each frame's grid made on the sidecar's device, a
-- wasm module importing wasi:webgpu, so it runs where the sidecar is granted
-- the GPU. With `memory` 'gpu' the picture stays on the GPU and one compute
-- pass reduces it to the grid, in the same integer arithmetic, so only the
-- grid's 1024 cells come back; with 'host' the frame is fetched and reduced
-- here. The rows are `simple_detector`'s.
CREATE FUNCTION simple_detector_gpu(v video_stream, threshold number DEFAULT 12,
                                    memory text DEFAULT 'gpu')
RETURNS STRUCT(start_t number, shot number)[]
  AS 'target/wasm32-wasip2/release/simple_detector_gpu.wasm', 'simple_detector'
  LANGUAGE wasm;
