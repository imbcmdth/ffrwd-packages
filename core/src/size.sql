-- One width, and the height worked out from it. `scale` derives a dimension
-- given a negative one, and the 2 in -2 is what keeps the result even, which
-- is what every 4:2:0 encoder downstream wants. Spelled out, `scale(v, w, -2)`
-- is the expression the recipes on the shelf repeat more than any other, and
-- the -2 is the part of it people forget.
--
-- It composes with anything holding a video stream. A ladder rung is this
-- function over a list of widths, a contact sheet is this function and a
-- still, a motion thumbnail is this function and an `fps`. The width is the
-- caller's; nothing here has a view on which ones are sensible.
CREATE FUNCTION to_width(v video_stream, width number)
RETURNS video_stream AS $$
  SELECT scale(v, width, -2)
$$ LANGUAGE sql;
