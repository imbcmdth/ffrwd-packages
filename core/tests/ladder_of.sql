-- The record-per-rung spelling has to reach the same command as the lists do.
COPY (
  WITH vid AS (
    SELECT l.v, l.rung, l.bitrate, l.bufsize
    FROM input('tests/fixtures/av.mp4') f,
         ffrwd.core.ladder_of(fps(f.video[1], 15),
                              ARRAY[STRUCT(640 AS width, '800k' AS bitrate, '1600k' AS bufsize),
                                    STRUCT(320 AS width, '300k' AS bitrate,  '600k' AS bufsize),
                                    STRUCT(160 AS width, '120k' AS bitrate,  '240k' AS bufsize)]) l
  )
  SELECT vid.v FROM vid
) TO 'out/master.m3u8'
  WITH (format 'hls', hls_time 2, hls_playlist_type 'vod', video_codec 'libx264',
        video_bitrate vid.bitrate, maxrate vid.bitrate, bufsize vid.bufsize)
