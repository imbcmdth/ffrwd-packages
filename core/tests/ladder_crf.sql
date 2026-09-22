-- No bitrates and no bufsizes: the rungs are widths and the quality target is
-- the destination's.
COPY (
  WITH vid AS (
    SELECT l.v, l.rung FROM input('tests/fixtures/av.mp4') f,
         ffrwd.core.ladder(f.video[1], ARRAY[640, 320]) l
  )
  SELECT vid.v FROM vid
) TO ('out-' || vid.rung::text || '.mp4') WITH (video_codec 'libx264', crf 23)
