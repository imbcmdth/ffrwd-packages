-- N evenly spaced stills, one image file each; raise `count` and the same query writes more.
-- variables: source (input media path), count (how many stills), width (still width in pixels, default 640), prefix (output name prefix, e.g. still-)
-- example: ffrwd compile -f packages/ffrwd/examples/recipes/contact-sheet.sql -v source=in.mp4 -v count=6 -v prefix=still-
COPY (
  SELECT scale(f.video[1], COALESCE(:width, 640), -2)
  FROM input(:'source') f, generate_series(1, :count) i
  WHERE f.t >= f.duration * (i.i - 0.5) / :count
) TO (:'prefix' || i.i::text || '.png') WITH (video_codec 'png', frames 1)
