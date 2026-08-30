-- Melt the far field: blur where the scene is far away, keeping what is
-- near sharp. The depth matte is near-bright, so it is negated to weigh the
-- blur toward the distance.
-- variables: source (input media path), sigma (blur strength, defaults to 12), track (video track index, defaults to the first), dest (output path)
-- example: ffrwd compile -f packages/ffrwd/depth/recipes/bokeh.sql -v source=street.mp4 -v dest=bokeh.mp4
COPY (
  SELECT ffrwd.mask_tools.blur_where(v, negate(ffrwd.depth.depth(v)), :sigma), f.audio
  FROM input(:'source') f, unnest(f.video) v
  WHERE v.index = COALESCE(:track, 1)
) TO :'dest' WITH (video_codec 'libx264', crf 20)
