COPY (
  SELECT scale(f.video[1], 320, -2)
  FROM input('tests/fixtures/av.mp4') f
) TO 'small.mp4' WITH (video_codec 'libx264')
