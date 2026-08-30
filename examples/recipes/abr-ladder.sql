-- Rendition ladder from list variables: one decode, one rung per series row, the rungs on the command line.
-- variables: source (input media path), prefix (output name prefix, e.g. out-), rungs (how many rungs, matching the two lists), widths (comma list of rung widths, e.g. 1920,1280,854), names (comma list of rung names, e.g. 1080p,720p,480p), crf (quality target for every rung, lower is bigger/better, defaults to the encoder's own)
-- example: ffrwd compile -f packages/ffrwd/examples/recipes/abr-ladder.sql -v source=in.mp4 -v prefix=out- -v rungs=3 -v widths=1920,1280,854 -v names=1080p,720p,480p -v crf=21
COPY (
  SELECT scale(f.video[1], :widths[i.i], -2) AS v, f.audio
  FROM input(:'source') f, generate_series(1, :rungs) i
) TO (:'prefix' || :'names'[i.i] || '.mp4')
  WITH (video_codec 'libx264', crf :crf, audio_codec 'aac')
