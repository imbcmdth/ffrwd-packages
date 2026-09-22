-- The ladder from parallel lists, against the ladder written out by hand.
COPY (
  WITH vid AS (
    SELECT l.v, l.rung, l.bitrate, l.bufsize
    FROM input('tests/fixtures/av.mp4') f,
         ffrwd.core.ladder(fps(f.video[1], 15), ARRAY[640, 320, 160],
                           ARRAY['800k', '300k', '120k'],
                           ARRAY['1600k', '600k', '240k']) l
  )
  SELECT vid.v FROM vid
) TO 'out/master.m3u8'
  WITH (format 'hls', hls_time 2, hls_playlist_type 'vod', video_codec 'libx264',
        video_bitrate vid.bitrate, maxrate vid.bitrate, bufsize vid.bufsize)
