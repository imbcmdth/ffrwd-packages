# ffrwd/examples

Eight common and fun recipes! These are the kind of jobs people reach for first, with variables named. Run them as they are or use them as a base for your own experiments.

```
ffrwd install ffrwd/examples
```

## The shelf

- **abr-ladder** - one input, many renditions, one command.
- **hls-ladder** - the same ladder as an HLS master playlist, video rungs and audio renditions demuxed.
- **dash-ladder** - the same again as a DASH manifest.
- **to-mp4** - any input down to one mp4: useful for dash or hls since it chooses the highest video rendition plus an audio track by `language`.
- **poster** - one poster frame at `at` seconds.
- **contact-sheet** - `count` evenly spaced stills, one image each.
- **motion-thumbnail** - `count` event spaced short clips, joined into one small mp4.
- **motion-thumbnail-gif** - the same sampled clips as a palette-optimized GIF.

## Running an installed recipe

Once installed, a recipe is addressed by name, no path, no checkout:

```
ffrwd run ffrwd.examples.poster -v source=in.mp4 -v at=90 -v dest=poster.png
```

`ffrwd list` shows every recipe with its required and optional
variables; an optional one left unset falls back to the default its
description names.

## What the recipes are made of

The functions are not here. `ffrwd/core` is where the ladder, the
audio renditions beside it, the width-only scale and the GIF palette
live, and its README is where each one is explained. What is on this
shelf is the starter wiring around them: the COPY, the destination and
the `WITH` that says what each row encodes to.

So a recipe is a good thing to copy and change. Change a width list and
you have your own ladder; change the destination and you have your own
job. If you find yourself writing the same expression in three of them,
that expression belongs in a package of your own, the way these ones
belong in core.
