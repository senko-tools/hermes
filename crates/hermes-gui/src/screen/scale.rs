//! Scaling captured pictures down, fast enough for 30 frames a second: an area average (every
//! source pixel counts by how much of it a target pixel covers, so text does not shimmer the
//! way it does with a few samples per pixel), done in two passes (across, then down) with 14-bit
//! fixed-point weights. Four channels per pixel, treated alike (BGRA or RGBA).

// Used by the (Windows-only) capture; tested everywhere.
#![cfg_attr(not(windows), allow(dead_code))]

const SHIFT: u32 = 14;
const ONE: u32 = 1 << SHIFT;

/// Which source pixels make up each target pixel, and how much each counts.
struct Taps {
    /// The first source pixel of each target pixel.
    start: Vec<usize>,
    /// Where each target pixel's weights begin in `weights` (one more entry at the end).
    offsets: Vec<usize>,
    /// Summing to [`ONE`] per target pixel.
    weights: Vec<u32>,
}

impl Taps {
    fn new(source: usize, target: usize) -> Taps {
        let scale = source as f64 / target as f64;
        let mut taps = Taps {
            start: Vec::with_capacity(target),
            offsets: Vec::with_capacity(target + 1),
            weights: Vec::new(),
        };
        for i in 0..target {
            let from = i as f64 * scale;
            let to = ((i + 1) as f64 * scale).min(source as f64);
            let first = from.floor() as usize;
            let last = (to.ceil() as usize).clamp(first + 1, source);
            taps.start.push(first);
            taps.offsets.push(taps.weights.len());
            let mut given = 0;
            for p in first..last {
                let covered = (to.min((p + 1) as f64) - from.max(p as f64)).max(0.0);
                let w = (covered / (to - from) * f64::from(ONE)).round() as u32;
                taps.weights.push(w);
                given += w;
            }
            // Rounding: the largest weight takes the difference, so the sum is exact.
            let mine = &mut taps.weights[taps.offsets[i]..];
            if let Some(max) = mine.iter_mut().max() {
                *max = (*max + ONE).saturating_sub(given);
            }
        }
        taps.offsets.push(taps.weights.len());
        taps
    }

    fn of(&self, i: usize) -> (usize, &[u32]) {
        (
            self.start[i],
            &self.weights[self.offsets[i]..self.offsets[i + 1]],
        )
    }
}

/// `pixels` (`width`×`height`, four bytes each) scaled down to `to_width`×`to_height` (each at
/// most the source size). Bands of rows are scaled on up to four threads.
pub fn down(pixels: &[u8], width: u32, height: u32, to_width: u32, to_height: u32) -> Vec<u8> {
    let (w, h) = (width as usize, height as usize);
    let (tw, th) = (to_width as usize, to_height as usize);
    debug_assert_eq!(pixels.len(), w * h * 4);
    if (w, h) == (tw, th) {
        return pixels.to_vec();
    }
    let threads = std::thread::available_parallelism()
        .map_or(1, std::num::NonZero::get)
        .clamp(1, 4);

    // Across: every source row to `tw` pixels.
    let across = Taps::new(w, tw);
    let mut narrow = vec![0u8; tw * h * 4];
    let band = h.div_ceil(threads);
    std::thread::scope(|scope| {
        for (src, dst) in pixels
            .chunks(band * w * 4)
            .zip(narrow.chunks_mut(band * tw * 4))
        {
            let across = &across;
            scope.spawn(move || scale_rows(src, dst, w, tw, across));
        }
    });

    // Down: rows of the narrowed picture, a whole row at a time (cache friendly).
    let down = Taps::new(h, th);
    let row = tw * 4;
    let mut out = vec![0u8; row * th];
    let band = th.div_ceil(threads);
    std::thread::scope(|scope| {
        for (i, dst) in out.chunks_mut(band * row).enumerate() {
            let (down, narrow) = (&down, &narrow);
            scope.spawn(move || scale_columns(narrow, dst, row, i * band, down));
        }
    });
    out
}

/// Scales whole rows (`src`, `w` pixels each) across to `tw` pixels each (`dst`).
fn scale_rows(src: &[u8], dst: &mut [u8], w: usize, tw: usize, taps: &Taps) {
    for (src, dst) in src.chunks_exact(w * 4).zip(dst.chunks_exact_mut(tw * 4)) {
        for (x, out) in dst.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let (first, weights) = taps.of(x);
            let mut acc = [ONE / 2; 4];
            for (k, wt) in weights.iter().enumerate() {
                let p = &src[(first + k) * 4..(first + k) * 4 + 4];
                for (a, v) in acc.iter_mut().zip(p) {
                    *a += u32::from(*v) * wt;
                }
            }
            for (o, a) in out.iter_mut().zip(acc) {
                *o = (a >> SHIFT) as u8;
            }
        }
    }
}

/// Makes the target rows from `first_row` on (`dst`, `row` bytes each) out of the source rows.
fn scale_columns(src: &[u8], dst: &mut [u8], row: usize, first_row: usize, taps: &Taps) {
    let mut acc = vec![0u32; row];
    for (y, dst) in dst.chunks_exact_mut(row).enumerate() {
        let (first, weights) = taps.of(first_row + y);
        acc.fill(ONE / 2);
        for (k, wt) in weights.iter().enumerate() {
            let src = &src[(first + k) * row..(first + k + 1) * row];
            for (a, p) in acc.iter_mut().zip(src) {
                *a += u32::from(*p) * wt;
            }
        }
        for (d, a) in dst.iter_mut().zip(&acc) {
            *d = (*a >> SHIFT) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weights_cover_each_target_pixel_exactly() {
        for (source, target) in [(2560, 1920), (1920, 1280), (3840, 1280), (7, 3), (5, 5)] {
            let taps = Taps::new(source, target);
            for i in 0..target {
                let (first, weights) = taps.of(i);
                assert_eq!(weights.iter().sum::<u32>(), ONE, "{source}->{target} #{i}");
                assert!(first + weights.len() <= source);
            }
        }
    }

    #[test]
    fn halving_averages_blocks() {
        // 4×2 pixels: a 2×2 block of 0 and 100, and one of 200 and 40.
        let px = |v: u8| [v, v, v, 255];
        let rows = [
            [px(0), px(100), px(200), px(40)],
            [px(100), px(0), px(40), px(200)],
        ];
        let pixels: Vec<u8> = rows.iter().flatten().flatten().copied().collect();
        let out = down(&pixels, 4, 2, 2, 1);
        assert_eq!(out, [50, 50, 50, 255, 120, 120, 120, 255]);
    }

    #[test]
    fn flat_colors_stay_flat() {
        let pixels: Vec<u8> = [12u8, 34, 56, 255].repeat(2560 * 1440);
        let out = down(&pixels, 2560, 1440, 1920, 1080);
        assert_eq!(out.len(), 1920 * 1080 * 4);
        assert!(
            out.as_chunks::<4>()
                .0
                .iter()
                .all(|p| *p == [12, 34, 56, 255])
        );
    }
}
