COPY (
  WITH vid AS (
    SELECT f.video[1] AS v, 1 AS rung FROM input('tests/fixtures/av.mp4') f
  ),
  aud AS (
    SELECT a AS t, a.index AS pos, 1 + a.index AS rung
    FROM input('tests/fixtures/av.mp4') g, unnest(g.audio) a
  )
  SELECT vid.v, aud.t FROM vid FULL JOIN aud ON vid.rung = aud.rung
) TO 'out/master.m3u8'
  WITH (format 'hls', hls_time 2, audio_codec 'aac',
        audio_bitrate ARRAY['192k', '128k'][aud.pos])
