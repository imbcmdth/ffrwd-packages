//! A fully-marked region has to come out as the overlay, exactly.
//!
//! WHY THIS IS AN EXEC-TIER TEST. The bug this guards against was not in any
//! arithmetic a unit test can reach: `src/composite.sql` names ffmpeg filters,
//! and what went wrong was ffmpeg's own pixel-format negotiation in the
//! process ffrwd puts the merge in. `maskedmerge` settles one format across
//! all its links and settles it by counting conversions; ffrwd hands two of
//! the three streams over a pipe as `rawvideo -pix_fmt yuv420p`, so the vote
//! went against the matte and the matte was converted to yuv on the way in.
//! Converted to yuv a grey matte is a luma and no colour at all: its weight
//! survived in the Y plane and both chroma planes arrived at the neutral 128
//! whatever the matte said, so colour was weighed 128/255 everywhere - at a
//! matte of 0 as much as at 255 - and about 14% of the original picture showed
//! through a region the matte had marked fully. Only a real ffmpeg, with the
//! streams arriving the way a pipe delivers them, can show that. So this test
//! renders frames.
//!
//! WHY IT DOES NOT GO THROUGH ffrwd. The shape that breaks needs the matte to
//! arrive from ANOTHER PROCESS, and in a plan of pure ffmpeg filters there is
//! no other process - several `input()`s compile to several `-i` on one
//! command, and all three `format()` calls then land together and agree. A
//! pipe boundary appears only when a wasm module is in the graph, and the
//! modules that rasterize a matte live in the packages that DEPEND on this
//! one. So the test reconstructs the consumer instead: three raw streams, two
//! of them yuv420p as a pipe delivers them and the matte in the sidecar's own
//! `rgba`, and the filter chain `masked` compiles to. `sql_still_spells_this`
//! is what keeps that chain honest - it fails if `src/composite.sql` is
//! re-spelled and this file is not re-derived from the new plan.
//!
//! It SKIPS, loudly, when ffmpeg cannot be run: the shelf's CI builds the wasm
//! and has no ffmpeg, and a build machine should not fail for that.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use composite::{block_spreads, differences, implied_alphas, median, Frame, Rect};

/// The frame every stream in this test is: a whole number of mosaic blocks on
/// both axes, and even, which yuv420p needs.
const WIDTH: usize = 160;
const HEIGHT: usize = 128;

/// The mosaic block the overlay is flat inside.
const BLOCK: usize = 16;

/// The region the matte marks fully, on a block boundary so that whole blocks
/// lie inside it.
const MARKED: Rect = Rect {
    x0: 16,
    y0: 16,
    x1: 144,
    y1: 112,
};

/// How far in from the matte's edge a measurement starts.
///
/// Right at the edge a 4:2:0 chroma cell straddles it and carries some of both
/// sides - the wire's own doing, since the two picture streams cross it
/// subsampled, and not something the composition can undo. Two pixels in is
/// past it.
const EDGE: usize = 2;

/// The chain `masked` compiles to in the process that consumes the matte:
/// input 0 the base, input 1 the overlay, input 2 the matte.
///
/// Kept in step with `src/composite.sql` by `sql_still_spells_this`.
const MASKED: &str = "[2:v:0]format=pix_fmts=gray[m];[1:v:0][m]alphamerge[o];[0:v:0][o]overlay[out0]";

/// The filters that chain names, which the SQL has to still name too.
const MASKED_FILTERS: [&str; 3] = ["format", "alphamerge", "overlay"];

// ---------------------------------------------------------------- the streams

/// A detailed picture as yuv420p bytes: nothing here is flat, so anything of
/// it that survives into a mosaic block shows up as spread.
fn detailed_yuv420p() -> Vec<u8> {
    let mut out = Vec::with_capacity(WIDTH * HEIGHT * 3 / 2);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            out.push(((x * 7 + y * 13) % 256) as u8);
        }
    }
    for plane in 0..2 {
        for y in 0..HEIGHT / 2 {
            for x in 0..WIDTH / 2 {
                out.push(((x * 11 + y * 5 + plane * 97) % 256) as u8);
            }
        }
    }
    out
}

/// A mosaic of that picture: one value per `BLOCK`-sized block, in luma and in
/// both chroma planes, so every block is flat before anything composites it.
fn mosaic_yuv420p() -> Vec<u8> {
    let value = |bx: usize, by: usize, salt: usize| ((bx * 23 + by * 61 + salt) % 256) as u8;
    let mut out = Vec::with_capacity(WIDTH * HEIGHT * 3 / 2);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            out.push(value(x / BLOCK, y / BLOCK, 0));
        }
    }
    for plane in 0..2 {
        for y in 0..HEIGHT / 2 {
            for x in 0..WIDTH / 2 {
                // A chroma sample sits at half resolution, so a block is
                // BLOCK/2 chroma samples wide and stays whole.
                out.push(value(x / (BLOCK / 2), y / (BLOCK / 2), 40 + plane * 90));
            }
        }
    }
    out
}

/// A matte as the sidecar writes one: the weight in red, green and blue and an
/// opaque alpha, which is what `rgba` carries on a stream edge.
fn matte_rgba(weight: impl Fn(usize, usize) -> u8) -> Vec<u8> {
    let mut out = Vec::with_capacity(WIDTH * HEIGHT * 4);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let w = weight(x, y);
            out.extend_from_slice(&[w, w, w, 255]);
        }
    }
    out
}

/// The same weights as a one-channel frame, to measure against.
fn matte_frame(weight: impl Fn(usize, usize) -> u8) -> Frame {
    let mut data = Vec::with_capacity(WIDTH * HEIGHT);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            data.push(weight(x, y));
        }
    }
    Frame::new(WIDTH, HEIGHT, 1, data).expect("the matte's own shape")
}

/// White inside `MARKED` and black outside it: a matte that says "replace
/// fully here, keep everything there" and nothing in between.
fn hard(x: usize, y: usize) -> u8 {
    if MARKED.contains(x, y) {
        255
    } else {
        0
    }
}

/// A ramp in steps `BLOCK` wide - the deliberate part of a `feather`, coarse
/// enough that each step is a whole number of chroma samples and so measures
/// without the subsampling wobble a per-pixel ramp would bring.
fn ramp(x: usize, _y: usize) -> u8 {
    let steps = WIDTH / BLOCK;
    ((x / BLOCK) * 255 / (steps - 1)) as u8
}

// ------------------------------------------------------------------- the rigs

/// ffmpeg, from `FFMPEG` if the environment names one, else off PATH. `None`
/// when it cannot be run at all, which is what makes this test skip.
fn ffmpeg() -> Option<PathBuf> {
    let named = std::env::var_os("FFMPEG").map_or_else(|| PathBuf::from("ffmpeg"), PathBuf::from);
    Command::new(&named)
        .arg("-version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|_| named)
}

/// A directory of one test's own under `target/`, emptied first so a rerun
/// never reads a stale frame. Named per test, since libtest runs the four of
/// them at once and they would otherwise clear each other's frames.
fn workspace(test: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(test);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("a directory under target/");
    dir
}

fn run(ff: &Path, args: &[&str]) {
    let done = Command::new(ff)
        .args(["-hide_banner", "-v", "error", "-y"])
        .args(args)
        .output()
        .expect("ffmpeg runs");
    assert!(
        done.status.success(),
        "ffmpeg {}\n{}",
        args.join(" "),
        String::from_utf8_lossy(&done.stderr)
    );
}

/// How a raw stream of `pix_fmt` is named as an input, the way a pipe's own
/// rawvideo arrives: carrying its pixel format and nothing about its colour.
fn raw_input<'a>(pix_fmt: &'a str, size: &'a str, path: &'a str) -> [&'a str; 8] {
    [
        "-f",
        "rawvideo",
        "-pixel_format",
        pix_fmt,
        "-video_size",
        size,
        "-i",
        path,
    ]
}

/// One rgb24 frame read back off disk.
fn frame(path: &Path) -> Frame {
    let bytes = fs::read(path).expect("the frame ffmpeg wrote");
    Frame::new(WIDTH, HEIGHT, 3, bytes).expect("one rgb24 frame of the expected size")
}

/// Render `graph` over the three streams and read the frame back.
fn compose(ff: &Path, dir: &Path, graph: &str, out: &str) -> Frame {
    let size = format!("{WIDTH}x{HEIGHT}");
    let (base, over, matte) = (
        dir.join("base.yuv"),
        dir.join("over.yuv"),
        dir.join("matte.rgba"),
    );
    let out = dir.join(out);
    let (b, o, m, o_s) = (
        base.to_string_lossy().to_string(),
        over.to_string_lossy().to_string(),
        matte.to_string_lossy().to_string(),
        out.to_string_lossy().to_string(),
    );
    let mut args: Vec<&str> = Vec::new();
    args.extend(raw_input("yuv420p", &size, &b));
    args.extend(raw_input("yuv420p", &size, &o));
    args.extend(raw_input("rgba", &size, &m));
    args.extend([
        "-filter_complex",
        graph,
        "-map",
        "[out0]",
        "-frames:v",
        "1",
        "-c:v",
        "rawvideo",
        "-pix_fmt",
        "rgb24",
        "-f",
        "rawvideo",
        &o_s,
    ]);
    run(ff, &args);
    frame(&out)
}

/// One of the picture streams on its own, converted to rgb24 by the same
/// swscale call the composed frame ends with.
///
/// This is the reference a fully-marked region owes: not the layer as it was
/// before it crossed the wire, but the layer as the consuming process has it.
fn layer(ff: &Path, dir: &Path, name: &str, out: &str) -> Frame {
    let size = format!("{WIDTH}x{HEIGHT}");
    let src = dir.join(format!("{name}.yuv"));
    let dst = dir.join(out);
    let (s, d) = (
        src.to_string_lossy().to_string(),
        dst.to_string_lossy().to_string(),
    );
    let mut args: Vec<&str> = Vec::new();
    args.extend(raw_input("yuv420p", &size, &s));
    args.extend([
        "-frames:v", "1", "-c:v", "rawvideo", "-pix_fmt", "rgb24", "-f", "rawvideo", &d,
    ]);
    run(ff, &args);
    frame(&dst)
}

/// The three streams written out, with `weight` deciding the matte.
fn streams(dir: &Path, weight: impl Fn(usize, usize) -> u8) {
    fs::write(dir.join("base.yuv"), detailed_yuv420p()).expect("write the base");
    fs::write(dir.join("over.yuv"), mosaic_yuv420p()).expect("write the overlay");
    fs::write(dir.join("matte.rgba"), matte_rgba(weight)).expect("write the matte");
}

// ------------------------------------------------------------------ the checks

/// A region the matte marks fully comes out as the overlay, exactly: not
/// nearly, not 86% of the way there.
///
/// This is the check the bug would have failed. Under the old `maskedmerge`
/// spelling the implied alpha here measured 0.86 and the interior was off the
/// overlay by up to 27 code values; the assertion asks for 1.0 and for zero.
#[test]
fn a_fully_marked_region_is_the_overlay_exactly() {
    let Some(ff) = ffmpeg() else {
        eprintln!("SKIPPED a_fully_marked_region_is_the_overlay_exactly: no ffmpeg to run");
        return;
    };
    let dir = workspace("a_fully_marked_region_is_the_overlay_exactly");
    streams(&dir, hard);

    let composed = compose(&ff, &dir, MASKED, "composed.rgb");
    let over = layer(&ff, &dir, "over", "over.rgb");
    let base = layer(&ff, &dir, "base", "base.rgb");
    let matte = matte_frame(hard);

    let inside = MARKED.inset(EDGE);
    let wrong = differences(&composed, &over, inside, 0);
    assert!(
        wrong.is_empty(),
        "{} of {} samples inside the fully-marked region are not the overlay; \
         worst at ({}, {}) channel {}: {} where the overlay has {}",
        wrong.len(),
        (inside.x1 - inside.x0) * (inside.y1 - inside.y0) * 3,
        wrong[0].x,
        wrong[0].y,
        wrong[0].channel,
        wrong[0].got,
        wrong[0].want,
    );

    let alphas = implied_alphas(&composed, &base, &over, &matte, 255, 25);
    let got = median(&alphas).expect("the two layers differ somewhere inside the matte");
    assert!(
        (got - 1.0).abs() < 1e-9,
        "a white matte weighed the overlay {got} rather than 1 ({} samples)",
        alphas.len()
    );
}

/// A mosaic block the overlay made flat stays flat: no spread means none of
/// the original picture is left inside it.
///
/// The symptom the bug was reported as. Spread is measured per channel,
/// because a range-converted matte weighs brightness and colour differently
/// and an average over the three would dilute it.
#[test]
fn a_flat_overlay_block_comes_out_flat() {
    let Some(ff) = ffmpeg() else {
        eprintln!("SKIPPED a_flat_overlay_block_comes_out_flat: no ffmpeg to run");
        return;
    };
    let dir = workspace("a_flat_overlay_block_comes_out_flat");
    streams(&dir, hard);

    let composed = compose(&ff, &dir, MASKED, "composed.rgb");
    let over = layer(&ff, &dir, "over", "over.rgb");

    let inside = MARKED.inset(EDGE);
    let (got, want) = (
        block_spreads(&composed, inside, BLOCK),
        block_spreads(&over, inside, BLOCK),
    );
    assert!(!got.is_empty(), "no whole block lies inside the matte");
    assert_eq!(
        got,
        want,
        "the composed picture is not as flat inside its blocks as the overlay \
         is: worst spread {} against the overlay's {}, over {} block-channels",
        got.iter().max().copied().unwrap_or(0),
        want.iter().max().copied().unwrap_or(0),
        got.len(),
    );
}

/// Everything the matte leaves black comes through as the base, exactly.
///
/// The other half of the same failure, and the half that is easy to miss: a
/// grey matte converted to yuv420p arrives with NO colour at all in its chroma
/// planes, 128 rather than 0, so the old spelling pulled the picture's colour
/// towards the overlay even where the matte said to keep it.
#[test]
fn an_unmarked_region_is_the_base_exactly() {
    let Some(ff) = ffmpeg() else {
        eprintln!("SKIPPED an_unmarked_region_is_the_base_exactly: no ffmpeg to run");
        return;
    };
    let dir = workspace("an_unmarked_region_is_the_base_exactly");
    streams(&dir, hard);

    let composed = compose(&ff, &dir, MASKED, "composed.rgb");
    let base = layer(&ff, &dir, "base", "base.rgb");

    let outside = MARKED.outset(EDGE, WIDTH, HEIGHT);
    let strips = [
        Rect { x0: 0, y0: 0, x1: WIDTH, y1: outside.y0 },
        Rect { x0: 0, y0: outside.y1, x1: WIDTH, y1: HEIGHT },
        Rect { x0: 0, y0: outside.y0, x1: outside.x0, y1: outside.y1 },
        Rect { x0: outside.x1, y0: outside.y0, x1: WIDTH, y1: outside.y1 },
    ];
    for strip in strips {
        let wrong = differences(&composed, &base, strip, 0);
        assert!(
            wrong.is_empty(),
            "{} samples outside the matte are not the base; worst at ({}, {}) \
             channel {}: {} where the base has {}",
            wrong.len(),
            wrong[0].x,
            wrong[0].y,
            wrong[0].channel,
            wrong[0].got,
            wrong[0].want,
        );
    }
}

/// A ramp still ramps: a feathered edge is deliberate, and the fix must not
/// have flattened it into a hard edge.
///
/// Each step of the ramp has to weigh the overlay by its own value. A
/// threshold would pass the two checks above and fail this one, which is why
/// it is here.
#[test]
fn a_feathered_ramp_weighs_by_the_matte() {
    let Some(ff) = ffmpeg() else {
        eprintln!("SKIPPED a_feathered_ramp_weighs_by_the_matte: no ffmpeg to run");
        return;
    };
    let dir = workspace("a_feathered_ramp_weighs_by_the_matte");
    streams(&dir, ramp);

    let composed = compose(&ff, &dir, MASKED, "composed.rgb");
    let over = layer(&ff, &dir, "over", "over.rgb");
    let base = layer(&ff, &dir, "base", "base.rgb");
    let matte = matte_frame(ramp);

    let mut levels: Vec<u8> = (0..WIDTH).map(|x| ramp(x, 0)).collect();
    levels.sort_unstable();
    levels.dedup();
    assert!(levels.len() >= 8, "the ramp has too few steps to say anything");

    let mut measured = Vec::new();
    for weight in levels {
        let alphas = implied_alphas(&composed, &base, &over, &matte, weight, 40);
        let got = median(&alphas).unwrap_or_else(|| panic!("no sample at weight {weight}"));
        let want = f64::from(weight) / 255.0;
        assert!(
            (got - want).abs() < 0.02,
            "a matte of {weight} weighed the overlay {got:.4} rather than {want:.4}"
        );
        measured.push(got);
    }
    assert!(
        measured.windows(2).all(|w| w[1] > w[0] - 1e-9),
        "the ramp does not rise monotonically: {measured:?}"
    );
}

/// The SQL still spells what this file renders.
///
/// The chain above was read off a compiled plan, and nothing makes it follow
/// `src/composite.sql` on its own. This asks the SQL whether it still names
/// the same filters, so a re-spelling fails here rather than leaving the rest
/// of the file quietly testing something the package no longer does.
#[test]
fn sql_still_spells_this() {
    let sql = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the package directory above this crate")
        .join("src")
        .join("composite.sql");
    let text = fs::read_to_string(&sql).expect("src/composite.sql is readable");
    let body = text
        .split_once("CREATE FUNCTION masked(")
        .expect("composite.sql declares masked")
        .1
        .split_once("$$ LANGUAGE sql")
        .expect("masked's body ends")
        .0;

    for filter in MASKED_FILTERS {
        assert!(
            body.contains(filter),
            "masked no longer names `{filter}`, which the chain in this file \
             renders: re-derive MASKED from a fresh `ffrwd compile` of a \
             consumer that reads a matte from a module, then update it here"
        );
    }
    assert!(
        !body.contains("maskedmerge"),
        "masked is spelled with `maskedmerge` again. It cannot hold: ffrwd \
         places two of the three streams behind a pipe that carries yuv420p, \
         so the merge settles on yuv420p and the matte is converted to it - \
         where a grey matte has no colour at all and both chroma planes arrive \
         at a neutral 128 whatever the matte said, weighing the picture's \
         colour half everywhere. See the comment at the top of \
         src/composite.sql"
    );
}
