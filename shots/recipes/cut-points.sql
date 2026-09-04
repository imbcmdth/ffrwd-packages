-- Every frame's shot index, one JSON row each, written straight to a rows
-- file: the shot number steps at each cut, so grep or jq turns this into a
-- cut list.
-- variables: source (input media path), threshold (cut sensitivity, defaults to 12), track (video track index, defaults to the first), dest (rows file, e.g. cuts.ndjson)
-- example: ffrwd compile -f packages/ffrwd/shots/recipes/cut-points.sql -v source=film.mp4 -v dest=cuts.ndjson
COPY (
  SELECT (ffrwd.shots.simple_detector(v, COALESCE(:threshold, 12))).cuts
  FROM input(:'source') f, unnest(f.video) v
  WHERE v.index = COALESCE(:track, 1)
) TO :'dest'
