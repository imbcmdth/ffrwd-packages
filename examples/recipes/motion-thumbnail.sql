-- Motion thumbnail: `count` clips spaced evenly through a file, joined into one small mp4.
-- variables: source (input media path), count (how many clips to sample), clip (seconds per clip, default 2), fps (frames per second, default 12), width (frame width in pixels, default 480), crf (quality target, lower is bigger/better, defaults to the encoder's own), dest (output .mp4 path)
-- example: ffrwd compile -f packages/ffrwd/examples/recipes/motion-thumbnail.sql -v source=film.mp4 -v count=5 -v dest=preview.mp4
COPY (
  WITH shots AS (
    SELECT fps(scale(f.video, COALESCE(:width, 480), -2), COALESCE(:fps, 12)) AS frame
    FROM input(:'source') f, generate_series(1, :count) i
    WHERE f.t >= f.duration * (i.i - 0.5) / :count - COALESCE(:clip, 2) / 2.0
      AND f.t <= f.duration * (i.i - 0.5) / :count + COALESCE(:clip, 2) / 2.0
  )
  SELECT ffmpeg.concat(VARIADIC array_agg(shots.frame))
  FROM shots
) TO :'dest' WITH (video_codec 'libx264', crf :crf)
