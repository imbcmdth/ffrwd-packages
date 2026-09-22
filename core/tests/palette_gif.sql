-- The palette counted over the frames that get written, and the map onto it.
COPY (
  SELECT ffrwd.core.palette_gif(fps(ffrwd.core.to_width(f.video[1], 320), 5))
  FROM input('tests/fixtures/av.mp4') f
) TO 'out.gif'
