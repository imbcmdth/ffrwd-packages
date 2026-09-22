-- A GIF carries 256 colours and no say in which, so a good one is made by
-- reading the clip twice: once to count a palette that suits it, once to map
-- the frames onto that palette. Written as one expression the two reads are
-- the same column, and the compiler inserts the split that feeds both halves
-- from one decode.
--
-- It composes at the end of a chain, after the scale and after the `fps`,
-- because the palette should be counted over the pixels that will actually be
-- written and not over the ones a later filter throws away.
CREATE FUNCTION palette_gif(frame video_stream)
RETURNS video_stream AS $$
  SELECT paletteuse(frame, palettegen(frame))
$$ LANGUAGE sql;
