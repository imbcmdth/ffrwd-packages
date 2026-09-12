//! What a matte is supposed to have done to a picture, measured.
//!
//! `src/composite.sql` promises three things of a matte: where it is white the
//! overlay replaces the base outright, where it is black the base comes
//! through untouched, and a feathered edge ramps between the two. This crate
//! is the arithmetic that checks those three on real frames -
//! `tests/fully_marked.rs` renders them with ffmpeg and calls in here.
//!
//! Everything is 8-bit interleaved RGB, which is what a rawvideo `rgb24` frame
//! is, and every measure is per channel: a matte converted into a picture
//! format weighs brightness and colour by different amounts, so an error
//! averaged over the three channels can hide in the average.

/// One frame: interleaved 8-bit samples, `channels` of them per pixel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub width: usize,
    pub height: usize,
    pub channels: usize,
    pub data: Vec<u8>,
}

impl Frame {
    /// A frame over raw interleaved bytes, or `None` if the bytes are not
    /// exactly one frame of that shape.
    pub fn new(width: usize, height: usize, channels: usize, data: Vec<u8>) -> Option<Self> {
        (data.len() == width * height * channels).then_some(Self {
            width,
            height,
            channels,
            data,
        })
    }

    /// The value of one channel at one pixel.
    pub fn at(&self, x: usize, y: usize, channel: usize) -> u8 {
        self.data[(y * self.width + x) * self.channels + channel]
    }

    /// Channel 0 read as a matte weight, which is what a grayscale matte
    /// carries in all of its channels.
    pub fn weight(&self, x: usize, y: usize) -> u8 {
        self.at(x, y, 0)
    }
}

/// A rectangle of pixels, in the half-open convention `x0..x1`, `y0..y1`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x0: usize,
    pub y0: usize,
    pub x1: usize,
    pub y1: usize,
}

impl Rect {
    /// The same rectangle pulled `n` pixels in on every side.
    ///
    /// Every measure of an interior takes this first. A matte edge is where a
    /// 4:2:0 chroma cell straddles the boundary and carries some of both
    /// sides, which is the wire's doing and not the composition's; two pixels
    /// in is past it.
    pub fn inset(self, n: usize) -> Self {
        Self {
            x0: self.x0 + n,
            y0: self.y0 + n,
            x1: self.x1.saturating_sub(n),
            y1: self.y1.saturating_sub(n),
        }
    }

    /// The same rectangle pushed `n` pixels out on every side, clamped to a
    /// frame of `width` by `height`.
    pub fn outset(self, n: usize, width: usize, height: usize) -> Self {
        Self {
            x0: self.x0.saturating_sub(n),
            y0: self.y0.saturating_sub(n),
            x1: (self.x1 + n).min(width),
            y1: (self.y1 + n).min(height),
        }
    }

    pub fn contains(self, x: usize, y: usize) -> bool {
        (self.x0..self.x1).contains(&x) && (self.y0..self.y1).contains(&y)
    }
}

/// One pixel that came out wrong, for a message that says where and by how
/// much rather than just that something did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mismatch {
    pub x: usize,
    pub y: usize,
    pub channel: usize,
    pub got: u8,
    pub want: u8,
}

/// Where `got` and `want` differ inside `region`, worst pixel first.
///
/// `tolerance` is the number of code values a difference is allowed to be, so
/// `0` asks for the bit-identical frame a fully-marked region owes.
pub fn differences(got: &Frame, want: &Frame, region: Rect, tolerance: u8) -> Vec<Mismatch> {
    let mut out = Vec::new();
    for y in region.y0..region.y1 {
        for x in region.x0..region.x1 {
            for c in 0..got.channels.min(want.channels) {
                let (g, w) = (got.at(x, y, c), want.at(x, y, c));
                if g.abs_diff(w) > tolerance {
                    out.push(Mismatch {
                        x,
                        y,
                        channel: c,
                        got: g,
                        want: w,
                    });
                }
            }
        }
    }
    out.sort_by_key(|m| std::cmp::Reverse(m.got.abs_diff(m.want)));
    out
}

/// The per-channel spread inside every whole `block`-sized block of `region`.
///
/// An overlay a mosaic made flat has to come out flat: a block whose spread is
/// not zero is one where something else - the base picture, weighed in by a
/// matte that did not mean 255 - is still showing through.
pub fn block_spreads(frame: &Frame, region: Rect, block: usize) -> Vec<u8> {
    let mut out = Vec::new();
    let mut y = region.y0.next_multiple_of(block);
    while y + block <= region.y1 {
        let mut x = region.x0.next_multiple_of(block);
        while x + block <= region.x1 {
            for c in 0..frame.channels {
                let (mut lo, mut hi) = (u8::MAX, u8::MIN);
                for dy in 0..block {
                    for dx in 0..block {
                        let v = frame.at(x + dx, y + dy, c);
                        lo = lo.min(v);
                        hi = hi.max(v);
                    }
                }
                out.push(hi - lo);
            }
            x += block;
        }
        y += block;
    }
    out
}

/// The alpha the composition actually used, solved per channel from
/// `composed = base + (overlay - base) * alpha`.
///
/// Only pixels where the two layers differ by more than `floor` are counted:
/// where they agree the equation says nothing about alpha. Pixels whose matte
/// weight is not `weight` are skipped, so a caller can ask about the white
/// region, the black one, or one step of a ramp.
pub fn implied_alphas(
    composed: &Frame,
    base: &Frame,
    overlay: &Frame,
    matte: &Frame,
    weight: u8,
    floor: u8,
) -> Vec<f64> {
    let mut out = Vec::new();
    for y in 0..composed.height {
        for x in 0..composed.width {
            if matte.weight(x, y) != weight {
                continue;
            }
            for c in 0..composed.channels {
                let (b, o) = (base.at(x, y, c) as f64, overlay.at(x, y, c) as f64);
                if (o - b).abs() <= floor as f64 {
                    continue;
                }
                out.push((composed.at(x, y, c) as f64 - b) / (o - b));
            }
        }
    }
    out
}

/// The median of a set of samples, or `None` when there are none.
pub fn median(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let mid = sorted.len() / 2;
    Some(if sorted.len().is_multiple_of(2) {
        (sorted[mid - 1] + sorted[mid]) / 2.0
    } else {
        sorted[mid]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame of `channels` channels where every pixel carries `value`.
    fn flat(width: usize, height: usize, channels: usize, value: u8) -> Frame {
        Frame::new(width, height, channels, vec![value; width * height * channels]).expect("shape")
    }

    #[test]
    fn a_frame_needs_exactly_its_own_bytes() {
        assert!(Frame::new(2, 2, 3, vec![0; 12]).is_some());
        assert!(Frame::new(2, 2, 3, vec![0; 11]).is_none());
        assert!(Frame::new(2, 2, 3, vec![0; 13]).is_none());
    }

    #[test]
    fn a_pixel_is_read_interleaved() {
        let f = Frame::new(2, 1, 3, vec![1, 2, 3, 4, 5, 6]).expect("shape");
        assert_eq!((f.at(0, 0, 0), f.at(0, 0, 2)), (1, 3));
        assert_eq!((f.at(1, 0, 0), f.at(1, 0, 2)), (4, 6));
        assert_eq!(f.weight(1, 0), 4, "the matte weight is channel 0");
    }

    #[test]
    fn inset_and_outset_stay_inside_the_frame() {
        let r = Rect {
            x0: 4,
            y0: 4,
            x1: 8,
            y1: 8,
        };
        assert_eq!(r.inset(2), Rect { x0: 6, y0: 6, x1: 6, y1: 6 });
        assert_eq!(r.inset(9), Rect { x0: 13, y0: 13, x1: 0, y1: 0 });
        assert_eq!(
            r.outset(10, 10, 10),
            Rect { x0: 0, y0: 0, x1: 10, y1: 10 },
            "outset clamps to the frame rather than running off it"
        );
    }

    #[test]
    fn differences_finds_the_worst_pixel_first() {
        let want = flat(2, 1, 3, 100);
        let mut got = want.clone();
        got.data[0] = 103;
        got.data[3] = 110;
        let whole = Rect { x0: 0, y0: 0, x1: 2, y1: 1 };
        assert!(differences(&got, &want, whole, 0).len() == 2);
        let worst = differences(&got, &want, whole, 0)[0];
        assert_eq!((worst.x, worst.got, worst.want), (1, 110, 100));
        assert_eq!(
            differences(&got, &want, whole, 3).len(),
            1,
            "a tolerance of 3 forgives the 3 and keeps the 10"
        );
        assert!(differences(&got, &want, whole, 10).is_empty());
    }

    #[test]
    fn a_flat_block_has_no_spread_and_one_pixel_of_base_gives_it_one() {
        let whole = Rect { x0: 0, y0: 0, x1: 4, y1: 4 };
        let flat_frame = flat(4, 4, 3, 60);
        assert_eq!(block_spreads(&flat_frame, whole, 4), vec![0, 0, 0]);

        let mut leaked = flat_frame.clone();
        leaked.data[(2 * 4 + 2) * 3 + 1] = 69;
        assert_eq!(
            block_spreads(&leaked, whole, 4),
            vec![0, 9, 0],
            "the leak is in the green channel alone and only that spread moves"
        );
    }

    #[test]
    fn block_spreads_only_measures_whole_blocks_of_the_region() {
        let frame = flat(8, 8, 1, 0);
        let whole = Rect { x0: 0, y0: 0, x1: 8, y1: 8 };
        assert_eq!(block_spreads(&frame, whole, 4).len(), 4);
        let ragged = Rect { x0: 1, y0: 1, x1: 8, y1: 8 };
        assert_eq!(
            block_spreads(&frame, ragged, 4).len(),
            1,
            "a block has to start on the grid and end inside the region"
        );
    }

    #[test]
    fn a_full_replacement_implies_an_alpha_of_one() {
        let base = flat(2, 1, 3, 40);
        let overlay = flat(2, 1, 3, 200);
        let matte = flat(2, 1, 3, 255);
        let alphas = implied_alphas(&overlay, &base, &overlay, &matte, 255, 25);
        assert_eq!(median(&alphas), Some(1.0));
        let alphas = implied_alphas(&base, &base, &overlay, &matte, 255, 25);
        assert_eq!(median(&alphas), Some(0.0), "the base alone implies nothing");
    }

    #[test]
    fn a_half_weighed_plane_is_what_an_implied_alpha_shows() {
        // What the old `maskedmerge` spelling did to the planes that carry
        // colour: a white matte converted to yuv has no colour in it, so both
        // chroma planes weighed by the neutral 128 rather than by 255. Solved
        // back out of the composed frame, that reads as an alpha of 128/255.
        let (base, overlay) = (flat(1, 1, 1, 0), flat(1, 1, 1, 255));
        let matte = flat(1, 1, 1, 255);
        let composed = flat(1, 1, 1, 128);
        let alphas = implied_alphas(&composed, &base, &overlay, &matte, 255, 25);
        let got = median(&alphas).expect("one sample");
        assert!((got - 128.0 / 255.0).abs() < 1e-9, "got {got}");
    }

    #[test]
    fn only_the_asked_for_matte_weight_is_counted() {
        let base = flat(2, 1, 1, 0);
        let overlay = flat(2, 1, 1, 200);
        let matte = Frame::new(2, 1, 1, vec![255, 128]).expect("shape");
        let composed = Frame::new(2, 1, 1, vec![200, 100]).expect("shape");
        assert_eq!(
            median(&implied_alphas(&composed, &base, &overlay, &matte, 255, 25)),
            Some(1.0)
        );
        assert_eq!(
            median(&implied_alphas(&composed, &base, &overlay, &matte, 128, 25)),
            Some(0.5)
        );
    }

    #[test]
    fn a_layer_pair_that_agrees_says_nothing_about_alpha() {
        let same = flat(2, 1, 3, 90);
        let matte = flat(2, 1, 3, 255);
        assert!(implied_alphas(&same, &same, &same, &matte, 255, 25).is_empty());
        assert_eq!(median(&[]), None);
    }

    #[test]
    fn median_of_an_even_count_is_the_middle_pair() {
        assert_eq!(median(&[4.0, 1.0, 3.0, 2.0]), Some(2.5));
        assert_eq!(median(&[3.0, 1.0, 2.0]), Some(2.0));
    }
}
