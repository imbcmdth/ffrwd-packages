-- The one model export, hosted as the wasm module the package ships. The
-- weights are pinned in the manifest and land beside the module at install.
--
-- `depth` returns the scene's monocular depth as a grayscale matte at the
-- frame's own geometry, near bright and far dark. There is no invert option:
-- ffmpeg's own `negate` filter is the inversion, so far-bright is spelled
-- `negate(ffrwd.depth.depth(v))`.
CREATE FUNCTION depth(v video_stream)
RETURNS video_stream
  AS 'target/wasm32-wasip2/release/depth.wasm', 'depth' LANGUAGE wasm;
