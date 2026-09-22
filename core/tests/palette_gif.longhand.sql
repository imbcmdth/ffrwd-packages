COPY (
  WITH shots AS (
    SELECT fps(scale(f.video[1], 320, -2), 5) AS frame FROM input('tests/fixtures/av.mp4') f
  )
  SELECT paletteuse(shots.frame, palettegen(shots.frame)) FROM shots
) TO 'out.gif'
