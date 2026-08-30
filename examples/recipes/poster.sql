-- One poster frame at a timestamp, from the first video track, or the one `track` names.
-- variables: source (input media path), at (seek time in seconds, default the first frame), track (video track index, defaults to the first), dest (output image path)
-- example: ffrwd compile -f packages/ffrwd/examples/recipes/poster.sql -v source=in.mp4 -v at=90 -v dest=poster.png
COPY (
  SELECT v FROM input(:'source') f, unnest(f.video) v
  WHERE v.index = COALESCE(:track, 1) AND f.t >= COALESCE(:at, 0)
) TO :'dest' WITH (video_codec 'png', frames 1)
