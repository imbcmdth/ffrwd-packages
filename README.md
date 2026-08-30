# ffrwd packages

The official `ffrwd/*` shelf, MIT licensed. Each directory is one
package — the `ffrwd.json` the compiler reads, the SQL it names, and,
where a package ships wasm, the cargo workspace that builds it.
Install any of them with `ffrwd install ffrwd/<name>`; the registry
at ffrwd.video serves the published versions.

| package | what it is |
| --- | --- |
| `examples/` | the starter shelf, recipes only — a rendition ladder, manifest-to-mp4, poster frames, a contact sheet, motion thumbnails |
| `mask_tools/` | composition over a grayscale matte, pure SQL over native ffmpeg — blur, mosaic, spotlight or cut out what the matte marks |
| `depth/` | monocular depth as a matte, hosted in wasm — near bright, far dark, beside the picture for everything that reads a matte |

Two official packages live in their own repositories for licensing
reasons: `ffrwd/yolo26` (AGPL-3.0, after its weights) and `ffrwd/moq`
(Apache-2.0).

## Building the wasm

A package's modules build against the wit from the installed
`ffrwd/wasm` package:

```
ffrwd install -g ffrwd/wasm
cargo build --target wasm32-wasip2 --release
```

run from the package's own directory (today only `depth/` ships a
module). Publishing is `ffrwd publish` from the package directory,
which validates everything first.
