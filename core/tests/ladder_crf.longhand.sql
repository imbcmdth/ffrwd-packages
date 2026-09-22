COPY (
  WITH vid AS (
    SELECT scale(f.video[1], ARRAY[640, 320][i.i], -2) AS v, i.i AS rung
    FROM input('tests/fixtures/av.mp4') f, generate_series(1, 2) i
  )
  SELECT vid.v FROM vid
) TO ('out-' || vid.rung::text || '.mp4') WITH (video_codec 'libx264', crf 23)
