-- Dim everything but the near field: the depth matte is near-bright, so
-- what is close keeps its own brightness and the distance darkens.
-- variables: source (input media path), dim (how much the far field darkens, 0 to 1, defaults to 0.6), track (video track index, defaults to the first), dest (output path)
-- example: ffrwd compile -f packages/ffrwd/depth/recipes/near-spotlight.sql -v source=street.mp4 -v dest=spotlit.mp4
COPY (
  SELECT ffrwd.mask_tools.spotlight(v, ffrwd.depth.depth(v), :dim), f.audio
  FROM input(:'source') f, unnest(f.video) v
  WHERE v.index = COALESCE(:track, 1)
) TO :'dest' WITH (video_codec 'libx264', crf 20)
