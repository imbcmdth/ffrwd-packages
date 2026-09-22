-- A rendition ladder is one decode and one row per rung, and the only thing
-- that changes between one ladder and the next is the rungs. What is here is
-- the shape; the rungs arrive as arguments, so nothing in this package has an
-- opinion about what 720p ought to cost.
--
-- The settings come back as columns rather than as something done to the
-- stream, because a bitrate belongs to the encode and the encode belongs to
-- the destination. A caller writes `WITH (video_bitrate l.bitrate,
-- bufsize l.bufsize, ...)` and the compiler binds one value per row, which is
-- the same binding a hand-written ladder uses. That is what keeps these
-- functions a naming convenience and not a second way through the compiler.

-- The rungs as parallel lists, which is the spelling the command line can
-- reach: `ARRAY[:widths]` and `ARRAY[:'bitrates']` under
-- `-v widths=1920,1280,854 -v bitrates=6000k,3000k,1000k` expand to one
-- element per rung, so a recipe names its ladder without editing the query.
-- The count is the widths list's own length, so no caller says it twice.
--
-- `bitrates` and `bufsizes` are optional and a rung with neither encodes by
-- whatever else the destination sets, a `crf` for instance. Passed at all,
-- they have to run the length of `widths`: a shorter list is a subscript past
-- the end and the compiler says so, naming the length it found.
CREATE FUNCTION ladder(v video_stream, widths number[],
                       bitrates text[] DEFAULT NULL, bufsizes text[] DEFAULT NULL)
RETURNS TABLE(v video_stream, rung number, bitrate text, bufsize text) AS $$
  SELECT to_width(v, widths[i.i]) AS v, i.i AS rung,
         bitrates[i.i] AS bitrate, bufsizes[i.i] AS bufsize
  FROM generate_series(1, array_length(widths, 1)) i
$$ LANGUAGE sql;

-- The same ladder with each rung written as one record, a width beside the
-- settings that belong to it, which is the spelling to reach for when the
-- rungs live in the query rather than on the command line. Nothing tells this
-- one how many rungs there are either: the list says, and `r.index` is the
-- 1-based position every row table carries.
CREATE FUNCTION ladder_of(v video_stream,
                          rungs STRUCT(width number, bitrate text, bufsize text)[])
RETURNS TABLE(v video_stream, rung number, bitrate text, bufsize text) AS $$
  SELECT to_width(v, r.width) AS v, r.index AS rung,
         r.bitrate AS bitrate, r.bufsize AS bufsize
  FROM unnest(rungs) r
$$ LANGUAGE sql;

-- The audio half of the same ladder: one row per track of the input, each
-- with the bitrate its position names. `first_rung` is how many video rungs
-- there are, so the audio rungs number past them and the `FULL JOIN` on the
-- rung finds no match: the two halves arrive as separate rows, which is what
-- a demuxed variant map wants. `array_length` of the widths list the ladder
-- read is the number to pass.
--
-- The whole `audio` array of an input goes in, so the tracks keep the order
-- and the language tags they were probed with, and `bitrates` is read by
-- track position the way `widths` is read by rung.
CREATE FUNCTION audio_renditions(a audio_stream[], bitrates text[], first_rung number)
RETURNS TABLE(t audio_stream, rung number, bitrate text) AS $$
  SELECT x AS t, first_rung + x.index AS rung, bitrates[x.index] AS bitrate
  FROM unnest(a) x
$$ LANGUAGE sql;
