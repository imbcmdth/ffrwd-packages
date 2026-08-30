-- The depth matte itself: the picture replaced by its own depth, near
-- bright and far dark.
-- variables: source (input media path), track (video track index, defaults to the first), dest (output path)
-- example: ffrwd compile -f packages/ffrwd/depth/recipes/depth-map.sql -v source=street.mp4 -v dest=depth.mp4
COPY (
  SELECT ffrwd.depth.depth(v)
  FROM input(:'source') f, unnest(f.video) v
  WHERE v.index = COALESCE(:track, 1)
) TO :'dest' WITH (video_codec 'libx264', crf 20)
