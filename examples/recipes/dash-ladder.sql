-- A DASH ladder, demuxed: a rung per video size, a rendition per audio
-- track, and segments that cut at the same instant in every one of them.
-- A manifest binds many outputs under one name, so the relation stays
-- rows: each row is one entry of the adaptation sets - a video row is a
-- rung, an audio row a rendition, and a row carrying both would be a
-- muxed variant. Under format 'dash' the compiler owns the alignment -
-- keyframes pinned to segment boundaries, scene cuts off - and writes
-- the adaptation sets by transcribing the rows.
-- variables: source (input media path), dest (manifest path, e.g. out/master.mpd), rungs (how many video rungs, matching the four lists), widths (comma list of rung widths, e.g. 1920,1280,854,640), bitrates (comma list of per-rung bitrates, e.g. 6000k,3000k,1200k,600k), maxrates (comma list of per-rung rate caps, e.g. 6600k,3300k,1320k,660k), bufsizes (comma list of per-rung buffer sizes, e.g. 9000k,4500k,1800k,900k), abitrates (comma list of per-track audio bitrates, e.g. 192k,128k), segment (segment length in seconds, e.g. 6), fps (the ladder's frame rate, e.g. 30)
-- example: ffrwd run ffrwd/examples:dash-ladder -v source=film.mp4 -v dest=out/master.mpd -v rungs=4 -v widths=1920,1280,854,640 -v bitrates=6000k,3000k,1200k,600k -v maxrates=6600k,3300k,1320k,660k -v bufsizes=9000k,4500k,1800k,900k -v abitrates=192k,128k -v segment=6 -v fps=30
COPY (
  WITH vid AS (
    SELECT scale(fps(f.video[1], :fps), :widths[i.i], -2) AS v, i.i AS rung
    FROM input(:'source') f, generate_series(1, :rungs) i
  ),
  aud AS (
    SELECT a AS t, a.index AS pos, :rungs + a.index AS rung
    FROM input(:'source') g, unnest(g.audio) a
  )
  SELECT vid.v, aud.t
  FROM vid FULL JOIN aud ON vid.rung = aud.rung
) TO :'dest'
  WITH (format 'dash',
        seg_duration :segment,
        use_template true,
        use_timeline true,
        video_codec 'libx264',
        video_bitrate :'bitrates'[vid.rung],
        maxrate :'maxrates'[vid.rung],
        bufsize :'bufsizes'[vid.rung],
        audio_codec 'aac',
        audio_bitrate :'abitrates'[aud.pos])
