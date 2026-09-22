# ffrwd/core

The functions the starter recipes are made of, pure SQL over stock
ffmpeg. No modules, no models, nothing fetched at run time, and no
package to install underneath this one.

```
ffrwd install ffrwd/core
```

Everything here is a name for a spelling you could have written out.
Each one compiles to the command the longhand reaches, byte for byte,
and `tests/run.sh` is that claim checked: every function beside the
hand-written job it stands for.

There are no presets. A ladder does not know what 720p costs and a
scale does not know what a thumbnail is; the numbers are the caller's,
every one of them an argument.

## Exports

- `to_width(v, width)` - scaled to a width, the height derived and kept
  even. `scale(v, width, -2)`, which is the expression a shelf of
  recipes repeats more than any other and the one whose `-2` gets
  forgotten. The rungs of a ladder are this over a list of widths.
- `ladder(v, widths, bitrates DEFAULT NULL, bufsizes DEFAULT NULL)` -
  one row per rung: the scaled stream, the rung number, and the
  encoder settings that go with it. Returns
  `TABLE(v, rung, bitrate, bufsize)`.
- `ladder_of(v, rungs)` - the same ladder with each rung written as one
  `STRUCT(width, bitrate, bufsize)` instead of a column per list.
- `audio_renditions(a, bitrates, first_rung)` - the audio half of a
  ladder: one row per track of the input, numbered past the video
  rungs. Returns `TABLE(t, rung, bitrate)`.
- `palette_gif(frame)` - the two-pass GIF palette, `palettegen` counted
  over the frames `paletteuse` then maps.

## The ladder

A rendition ladder is one decode and one row per rung, and the only
thing that changes between one ladder and the next is the rungs. So the
rungs are arguments and the function is the shape:

```sql
COPY (
  SELECT l.v
  FROM input(:'source') f,
       ffrwd.core.ladder(f.video[1], ARRAY[:widths],
                         ARRAY[:'bitrates'], ARRAY[:'bufsizes']) l
) TO :'dest'
  WITH (format 'hls', hls_time 6, video_codec 'libx264',
        video_bitrate l.bitrate, maxrate l.bitrate, bufsize l.bufsize)
```

Nothing tells it how many rungs there are: the widths list says, and
`-v widths=1920,1280,854 -v bitrates=6000k,3000k,1000k` on the command
line fills all three lists at once, one element per rung. `ARRAY[:'name']`
is what expands a comma list into one string literal per element, so a
caller names the ladder without editing the query. It needs ffrwd 0.18.3.

The settings come back as columns rather than as something done to the
stream, because a bitrate belongs to the encode and the encode belongs
to the destination. `video_bitrate l.bitrate` binds one value per row,
which is the same binding a hand-written ladder uses.

`bitrates` and `bufsizes` are optional, and a ladder with neither is a
ladder of widths that encodes by whatever else the destination sets, a
`crf` for instance. Passed at all, they run the length of `widths`: a
shorter list is a subscript past the end, and the compiler says so and
names the length it found.

`ladder_of` is the spelling to reach for when the rungs live in the
query, a width beside the settings that belong to it:

```sql
ffrwd.core.ladder_of(f.video[1],
                     ARRAY[STRUCT(1920 AS width, '6000k' AS bitrate, '9000k' AS bufsize),
                           STRUCT(1280 AS width, '3000k' AS bitrate, '4500k' AS bufsize)])
```

## The audio beside it

A demuxed manifest wants the audio as its own rows, so the two halves
are two relations joined on a rung number that never matches:

```sql
WITH vid AS (
  SELECT l.v, l.rung, l.bitrate, l.bufsize
  FROM input(:'source') f,
       ffrwd.core.ladder(f.video[1], ARRAY[:widths], ARRAY[:'bitrates'], ARRAY[:'bufsizes']) l
),
aud AS (
  SELECT r.t, r.rung, r.bitrate
  FROM input(:'source') g,
       ffrwd.core.audio_renditions(g.audio, ARRAY[:'abitrates'],
                                   array_length(ARRAY[:widths], 1)) r
)
SELECT vid.v, aud.t FROM vid FULL JOIN aud ON vid.rung = aud.rung
```

`first_rung` is how many video rungs there are, which is
`array_length` of the same widths list the ladder read. Past them, the
`FULL JOIN` finds no match and every row comes out one-sided: a video
row is a variant drawing from the audio group, an audio row a rendition
in it. The whole `audio` array goes in, so the tracks keep the order and
the language tags they were probed with.

## Recipes

There are none here. `ffrwd/examples` is the starter shelf, and every
recipe on it is written over these functions.

## Tests

```
ffrwd link
bash tests/run.sh
```

It makes a five-second lavfi fixture under `tests/fixtures/` the first
time, then compiles each function beside its longhand and compares the
two commands. Nothing is encoded and nothing is fetched.
