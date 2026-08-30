# ffrwd/examples

Six recipes and nothing else: no wasm, no exports, nothing to build.
This is the starter shelf — the jobs people reach for first, each one
a single SQL file short enough to read whole, with its variables named
in a header comment. Run them as they are, or copy one out and bend it.

```
ffrwd install ffrwd/examples
```

## The shelf

- **abr-ladder** — one input, three renditions, one command. The rungs
  are rows in an inline table; add a rung by adding a row.
- **to-mp4** — a DASH/HLS manifest (or any input) down to one mp4: the
  widest video rendition plus the audio track `language` names, both
  chosen at compile time.
- **poster** — one poster frame at `at` seconds.
- **contact-sheet** — `count` evenly spaced stills, one image each.
- **motion-thumbnail** — `count` short clips sampled evenly through
  the file, joined into one small mp4 preview.
- **motion-thumbnail-gif** — the same sampled clips as a
  palette-optimized GIF.

## A manifest down to one mp4

A streaming manifest lists every rendition and every language. The
recipe reads them as rows, keeps the widest video and the audio track
you name, and stream-copies both into a plain mp4 — no re-encode, and
the choice is made before ffmpeg starts:

```
$ ffrwd compile -f recipes/to-mp4.sql \
    -v source=https://storage.googleapis.com/shaka-demo-assets/angel-one/dash.mpd \
    -v language=de -v dest=out.mp4
ffmpeg -i https://storage.googleapis.com/shaka-demo-assets/angel-one/dash.mpd -map 0:v:4 -c:0 copy -map 0:a:1 -c:1 copy -metadata:s:1 language=de out.mp4
```

That manifest carries ten video renditions and five audio languages;
the compiler probed it, sorted the renditions, and mapped exactly two
streams. Set `vcodec`/`acodec` to transcode instead of copy.

## A ladder from a table

The rung table is written in the query itself — `unnest(ARRAY[STRUCT(...)])`
is a row per rung, and each row keys its own scale and its own output
file. One decode feeds all three encodes:

```pgsql
COPY (
  SELECT scale(f.video[1], r.w, -2) AS v, f.audio
  FROM input(:'source') f,
       unnest(ARRAY[STRUCT(1920 AS w, '1080p' AS name),
                    STRUCT(1280 AS w, '720p' AS name),
                    STRUCT(854 AS w, '480p' AS name)]) r
) TO (:'prefix' || r.name || '.mp4')
  WITH (video_codec 'libx264', crf :crf, audio_codec 'aac')
```

```
$ ffrwd compile -f recipes/abr-ladder.sql -v source=in.mp4 -v prefix=out- -v crf=21
ffmpeg -i in.mp4 -filter_complex '[0:v:0]split=3[src_f_v_0_split0][src_f_v_0_split1][src_f_v_0_split2];[src_f_v_0_split0]scale=width=1920:height=-2[out0];[src_f_v_0_split1]scale=width=1280:height=-2[out2];[src_f_v_0_split2]scale=width=854:height=-2[out4]' -map '[out0]' -map 0:a:0 -c:0 libx264 -crf:0 21 -c:1 aac out-1080p.mp4 -map '[out2]' -map 0:a:0 -c:0 libx264 -crf:0 21 -c:1 aac out-720p.mp4 -map '[out4]' -map 0:a:0 -c:0 libx264 -crf:0 21 -c:1 aac out-480p.mp4
```

The `split=3` is the compiler's own: the source is decoded once and
fanned out, because three consumers of one stream need one.

## Running an installed recipe

Once installed, a recipe is addressed by name — no path, no checkout:

```
ffrwd run ffrwd.examples.poster -v source=in.mp4 -v at=90 -v dest=poster.png
```

`ffrwd list` shows every recipe with its required and optional
variables; an optional one left unset falls back to the default its
description names.
