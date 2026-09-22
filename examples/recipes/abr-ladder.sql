-- Rendition ladder from list variables: one decode, one rung per width, the rungs on the command line.
-- variables: source (input media path), prefix (output name prefix, e.g. out-), widths (comma list of rung widths, e.g. 1920,1280,854), names (comma list of rung names, one per width, e.g. 1080p,720p,480p), crf (quality target for every rung, lower is bigger/better, defaults to the encoder's own), bitrates (comma list of per-rung bitrates, one per width, e.g. 6000k,3000k,1000k)
-- example: ffrwd compile -f packages/ffrwd/examples/recipes/abr-ladder.sql -v source=in.mp4 -v prefix=out- -v widths=1920,1280,854 -v names=1080p,720p,480p -v bitrates=6000k,3000k,1000k
COPY (
  SELECT l.v, f.audio
  FROM input(:'source') f,
       ffrwd.core.ladder(f.video[1], ARRAY[:widths], ARRAY[:'bitrates']) l
) TO (:'prefix' || :'names'[l.rung] || '.mp4')
  WITH (video_codec 'libx264', crf :crf, video_bitrate l.bitrate, audio_codec 'aac')
