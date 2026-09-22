-- to_width is scale to a width with the height derived and kept even.
COPY (
  SELECT ffrwd.core.to_width(f.video[1], 320)
  FROM input('tests/fixtures/av.mp4') f
) TO 'small.mp4' WITH (video_codec 'libx264')
