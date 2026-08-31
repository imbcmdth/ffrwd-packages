# ffrwd/examples

Six commonly and fun recipes! These are the kind of jobs people reach for first, with variables named. Run them as they are or use them as a base for your own experiments.

```
ffrwd install ffrwd/examples
```

## The shelf

- **abr-ladder** — one input, many renditions, one command.
- **to-mp4** — any input down to one mp4: useful for dash or hls since it chooses the highest video rendition plus an audio track by `language`.
- **poster** — one poster frame at `at` seconds.
- **contact-sheet** — `count` evenly spaced stills, one image each.
- **motion-thumbnail** — `count` event spaced short clips, joined into one small mp4.
- **motion-thumbnail-gif** — the same sampled clips as a palette-optimized GIF.

## Running an installed recipe

Once installed, a recipe is addressed by name — no path, no checkout:

```
ffrwd run ffrwd.examples.poster -v source=in.mp4 -v at=90 -v dest=poster.png
```

`ffrwd list` shows every recipe with its required and optional
variables; an optional one left unset falls back to the default its
description names.
