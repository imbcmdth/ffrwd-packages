-- Composition over a matte, spelled once and reused. Everything here is
-- native ffmpeg: any grayscale matte beside any stream goes in, and the
-- filtering it selects happens in the encoder's own graph.
--
-- `maskedmerge` takes base, overlay, mask in that pad order and shows the
-- overlay where the mask is white. Its planes weigh independently, so the
-- three streams are brought to gbrp first: a grayscale matte then lands
-- equal in all three planes and one mask weighs the whole picture.
CREATE FUNCTION masked(v video_stream, matte video_stream, filtered video_stream)
RETURNS video_stream AS $$
  SELECT maskedmerge(ffmpeg.format(v, 'gbrp'), ffmpeg.format(filtered, 'gbrp'),
                     ffmpeg.format(matte, 'gbrp'))
$$ LANGUAGE sql;

CREATE FUNCTION blur_where(v video_stream, matte video_stream, sigma number DEFAULT 12)
RETURNS video_stream AS $$
  SELECT masked(v, matte, gblur(v, sigma))
$$ LANGUAGE sql;

CREATE FUNCTION mosaic_where(v video_stream, matte video_stream, size number DEFAULT 16)
RETURNS video_stream AS $$
  SELECT masked(v, matte, pixelize(v, size, size))
$$ LANGUAGE sql;

-- Everything but the matte darkened: the merge keeps the picture where the
-- matte is white and takes the dimmed copy elsewhere.
CREATE FUNCTION spotlight(v video_stream, matte video_stream, dim number DEFAULT 0.6)
RETURNS video_stream AS $$
  SELECT masked(colorchannelmixer(v, rr => 1 - dim, gg => 1 - dim, bb => 1 - dim), matte, v)
$$ LANGUAGE sql;

-- The matte as an alpha channel, laid over a new background.
CREATE FUNCTION cutout(v video_stream, matte video_stream, background video_stream)
RETURNS video_stream AS $$
  SELECT ffmpeg.overlay(background,
                        alphamerge(ffmpeg.format(v, 'rgba'), ffmpeg.format(matte, 'gray')))
$$ LANGUAGE sql;
