-- Motion thumbnail as a GIF: the same evenly spaced clips, each mapped through its own palette.
-- The palette halves read the clip twice; the compiler inserts the split.
-- variables: source (input media path), count (how many clips to sample), clip (seconds per clip, default 2), fps (frames per second, default 5), width (frame width in pixels, default 480), dest (output .gif path)
-- example: ffrwd compile -f packages/ffrwd/examples/recipes/motion-thumbnail-gif.sql -v source=film.mp4 -v count=5 -v dest=preview.gif
CREATE FUNCTION palette_gif(frame video_stream)
RETURNS video_stream AS $$
  SELECT paletteuse(frame, palettegen(frame))
$$ LANGUAGE sql;

COPY (
  WITH shots AS (
    SELECT palette_gif(
             fps(scale(f.video, COALESCE(:width, 480), -2), COALESCE(:fps, 5))) AS frame
    FROM input(:'source') f, generate_series(1, :count) i
    WHERE f.t >= f.duration * (i.i - 0.5) / :count - COALESCE(:clip, 2) / 2.0
      AND f.t <= f.duration * (i.i - 0.5) / :count + COALESCE(:clip, 2) / 2.0
  )
  SELECT ffmpeg.concat(VARIADIC array_agg(shots.frame))
  FROM shots
) TO :'dest'
