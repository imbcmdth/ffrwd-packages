-- One row per audio track, rungs numbered past the video ones so the join
-- leaves the two halves side by side.
COPY (
  WITH vid AS (
    SELECT f.video[1] AS v, 1 AS rung FROM input('tests/fixtures/av.mp4') f
  ),
  aud AS (
    SELECT r.t, r.rung, r.bitrate
    FROM input('tests/fixtures/av.mp4') g, ffrwd.core.audio_renditions(g.audio, ARRAY['192k', '128k'], 1) r
  )
  SELECT vid.v, aud.t FROM vid FULL JOIN aud ON vid.rung = aud.rung
) TO 'out/master.m3u8'
  WITH (format 'hls', hls_time 2, audio_codec 'aac', audio_bitrate aud.bitrate)
