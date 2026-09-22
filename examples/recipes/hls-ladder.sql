-- An HLS ladder, demuxed: a rung per video size, a rendition per audio
-- track, and segments that cut at the same instant in every one of them.
-- A manifest binds many outputs under one name, so the relation stays
-- rows: each row is one entry of the variant map - a video row is a
-- rung, an audio row a rendition, and a row carrying both would be a
-- muxed variant. Under format 'hls' the compiler owns the alignment -
-- keyframes pinned to segment boundaries, scene cuts off - and writes
-- the variant map by transcribing the rows.
-- The two halves are `ffrwd/core`'s ladder and its audio renditions; the
-- audio rungs number past the video ones, so the join leaves every row
-- one-sided.
-- variables: source (input media path), dest (master playlist path, e.g. out/master.m3u8), widths (comma list of rung widths, e.g. 1920,1280,854,640), bitrates (comma list of per-rung bitrates, one per width, e.g. 6000k,3000k,1200k,600k), maxrates (comma list of per-rung rate caps, one per width, e.g. 6600k,3300k,1320k,660k), bufsizes (comma list of per-rung buffer sizes, one per width, e.g. 9000k,4500k,1800k,900k), abitrates (comma list of per-track audio bitrates, e.g. 192k,128k), segment (segment length in seconds, e.g. 6), fps (the ladder's frame rate, e.g. 30)
-- example: ffrwd run ffrwd/examples:hls-ladder -v source=film.mp4 -v dest=out/master.m3u8 -v widths=1920,1280,854,640 -v bitrates=6000k,3000k,1200k,600k -v maxrates=6600k,3300k,1320k,660k -v bufsizes=9000k,4500k,1800k,900k -v abitrates=192k,128k -v segment=6 -v fps=30
COPY (
  WITH vid AS (
    SELECT l.v, l.rung, l.bitrate, l.bufsize
    FROM input(:'source') f,
         ffrwd.core.ladder(fps(f.video[1], :fps), ARRAY[:widths],
                           ARRAY[:'bitrates'], ARRAY[:'bufsizes']) l
  ),
  aud AS (
    SELECT r.t, r.rung, r.bitrate
    FROM input(:'source') g,
         ffrwd.core.audio_renditions(g.audio, ARRAY[:'abitrates'],
                                     array_length(ARRAY[:widths], 1)) r
  )
  SELECT vid.v, aud.t
  FROM vid FULL JOIN aud ON vid.rung = aud.rung
) TO :'dest'
  WITH (format 'hls',
        hls_time :segment,
        hls_playlist_type 'vod',
        hls_segment_type 'fmp4',
        video_codec 'libx264',
        video_bitrate vid.bitrate,
        maxrate :'maxrates'[vid.rung],
        bufsize vid.bufsize,
        audio_codec 'aac',
        audio_bitrate aud.bitrate)
