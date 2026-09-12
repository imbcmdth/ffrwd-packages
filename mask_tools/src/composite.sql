-- Composition over a matte, spelled once and reused. Everything here is
-- native ffmpeg: any grayscale matte beside any stream goes in, and the
-- filtering it selects happens in the encoder's own graph.
--
-- The matte is a weight, and it means what it says: white and the overlay
-- replaces the base outright, black and the base comes through untouched, a
-- `feather` edge ramps between the two. It rides in as an alpha channel.
-- `alphamerge` copies its luma into the overlay's alpha and `overlay` weighs
-- by that, equally in every plane, whichever way the compiler cuts the graph.
--
-- It used to be `maskedmerge`, and that was wrong in a way worth recording.
-- `maskedmerge` weighs its inputs plane by plane, so a grayscale matte has to
-- be fanned into all three planes first, which is what `gbrp` does. But
-- avfilter settles one pixel format across a filter's links and settles it by
-- counting conversions, and ffrwd hands two of the three streams over a pipe
-- as `rawvideo -pix_fmt yuv420p`, a `format()` on the far side of a pipe
-- being undone by the pipe. Two votes to one: the merge ran in yuv420p and
-- the matte was converted on the way in. A grey matte converted to yuv is a
-- luma and no colour at all. Its weight survived in the Y plane and both
-- chroma planes arrived at the neutral 128 whatever the matte said, so colour
-- was weighed 128/255 everywhere, at a matte of 0 as much as at 255.
--
-- What that looked like: through `ffrwd/censor`, an implied alpha of 0.88 in
-- rgb and a per-channel spread of 9 median and 40 worst inside blocks the
-- mosaic had made perfectly flat, so a face was still readable through its
-- own mosaic.
--
-- Nothing on the matte's side of the merge reaches it. Tagging the matte full
-- range, converting it with explicit full-range scaling, handing it over as
-- `gray`, and asking for `gbrp` on the merge's output all measure identically
-- to no change at all, because the conversion happens after whatever the
-- matte's own chain said; thresholding it to 0 and 255 first also flattens
-- the ramp a `feather` asks for. The one spelling that does fix the merge is
-- all three `format()` calls inside the consuming process, and that is not
-- available here: which process each one lands in is the compiler's choice.
-- The alpha path has none of this to survive. Measured, a fully-marked
-- interior comes out bit-identical to the overlay, an unmarked one
-- bit-identical to the base, and a feathered ramp weighs by the matte to
-- within a thousandth.
CREATE FUNCTION masked(v video_stream, matte video_stream, filtered video_stream)
RETURNS video_stream AS $$
  SELECT ffmpeg.overlay(v, alphamerge(filtered, ffmpeg.format(matte, 'gray')))
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

-- The matte as an alpha channel, laid over a new background, which is
-- `masked` read the other way round: the background is what the picture is
-- laid into and the matte says where.
--
-- This was already the alpha path. What it also carried was
-- `format(v, 'rgba')`, a conversion the compiler places upstream and the pipe
-- then undoes, so the picture made a trip out to full-range rgb and back
-- before anything looked at it. `alphamerge` asks for the alpha format it
-- needs itself, inside the process that runs it. Measured, the cut-out
-- interior went from 6 code values off the picture at worst to bit-identical
-- with it.
CREATE FUNCTION cutout(v video_stream, matte video_stream, background video_stream)
RETURNS video_stream AS $$
  SELECT masked(background, matte, v)
$$ LANGUAGE sql;
