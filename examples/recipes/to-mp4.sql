-- One mp4 from a DASH/HLS manifest (or any input): the widest video rendition plus the audio track `language` names. Stream copies unless codecs are set.
-- variables: source (input media path or manifest URL), language (audio language tag to keep, e.g. en), vcodec (video codec, defaults to copy), acodec (audio codec, defaults to copy), dest (output .mp4 path)
-- example: ffrwd compile -f packages/ffrwd/examples/recipes/to-mp4.sql -v source=in.mpd -v language=eng -v dest=out.mp4
COPY (
  WITH vid AS (
    SELECT t AS track
    FROM input(:'source') f, unnest(f.video) t
    ORDER BY t.width DESC LIMIT 1
  ),
  aud AS (
    SELECT a AS track
    FROM input(:'source') g, unnest(g.audio) a
    WHERE a.tags.language = :'language'
    ORDER BY a.index LIMIT 1
  )
  SELECT vid.track, aud.track FROM vid, aud
) TO :'dest' WITH (video_codec :'vcodec', audio_codec :'acodec')
