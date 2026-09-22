COPY (
  WITH vid AS (
    SELECT scale(fps(f.video[1], 15), ARRAY[640, 320, 160][i.i], -2) AS v, i.i AS rung
    FROM input('tests/fixtures/av.mp4') f, generate_series(1, 3) i
  )
  SELECT vid.v FROM vid
) TO 'out/master.m3u8'
  WITH (format 'hls', hls_time 2, hls_playlist_type 'vod', video_codec 'libx264',
        video_bitrate ARRAY['800k', '300k', '120k'][vid.rung],
        maxrate ARRAY['800k', '300k', '120k'][vid.rung],
        bufsize ARRAY['1600k', '600k', '240k'][vid.rung])
