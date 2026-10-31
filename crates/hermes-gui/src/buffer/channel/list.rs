//! Virtualization of the message list: only the rows near the visible part are built, with
//! spacers for the rest. Rows have different heights, so each row starts with an estimate and
//! keeps its measured height once shown. The list is anchored to the bottom, so offsets count
//! from the bottom.

use std::collections::HashMap;
use std::ops::Range;

use super::rows::RowKey;

/// Rows this far outside the viewport are built too, so scrolling shows no gaps.
pub const OVERSCAN: f32 = 600.0;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Heights {
    measured: HashMap<RowKey, f32>,
}

impl Heights {
    pub fn get(&self, key: &RowKey) -> Option<f32> {
        self.measured.get(key).copied()
    }

    /// Stores a measured height; `true` if it changed.
    pub fn set(&mut self, key: RowKey, height: f32) -> bool {
        let old = self.measured.insert(key, height);
        old.is_none_or(|h| (h - height).abs() > 0.5)
    }

    /// Forgets everything (the width changed, so every row may wrap differently).
    pub fn clear(&mut self) {
        self.measured.clear();
    }
}

/// What to build: the rows in `range`, a spacer of `above` pixels before them and `below`
/// after them.
#[derive(Debug, Clone, PartialEq)]
pub struct Visible {
    pub range: Range<usize>,
    pub above: f32,
    pub below: f32,
}

/// The rows (top to bottom, with `heights`) that overlap the viewport, `from_bottom` pixels
/// above the bottom and `viewport` pixels high, plus [`OVERSCAN`] on both sides.
pub fn visible(heights: &[f32], from_bottom: f32, viewport: f32) -> Visible {
    let low = (from_bottom - OVERSCAN).max(0.0);
    let high = from_bottom + viewport + OVERSCAN;
    let mut below = 0.0;
    let mut end = heights.len();
    let mut start = heights.len();
    let mut acc = 0.0;
    for (i, h) in heights.iter().enumerate().rev() {
        let (bottom, top) = (acc, acc + h);
        acc = top;
        if top <= low {
            below += h;
            end = i;
            start = i;
        } else if bottom < high {
            start = i;
        } else {
            break;
        }
    }
    let shown: f32 = heights[start..end].iter().sum();
    Visible {
        range: start..end,
        above: (acc_total(heights) - below - shown).max(0.0),
        below,
    }
}

fn acc_total(heights: &[f32]) -> f32 {
    heights.iter().sum()
}

/// A first guess for a message row before it is measured.
pub fn estimate_message(
    content: &str,
    grouped: bool,
    attachments: &[(Option<u32>, Option<u32>, bool)],
    reactions: bool,
    reply: bool,
    width: f32,
    font_size: f32,
) -> f32 {
    let line = font_size * 1.4;
    let chars_per_line = ((width - 80.0).max(100.0) / (font_size * 0.55)).max(10.0);
    let lines: f32 = content
        .split('\n')
        .map(|l| (l.chars().count() as f32 / chars_per_line).ceil().max(1.0))
        .sum();
    let mut h = 8.0 + lines * line;
    if !grouped {
        h += line + 4.0;
    }
    if reply {
        h += line;
    }
    for (w, ht, image) in attachments {
        h += match (image, w, ht) {
            (true, Some(w), Some(ht)) if *w > 0 => {
                let scale = (400.0 / *w as f32).min(300.0 / *ht as f32).min(1.0);
                *ht as f32 * scale + 8.0
            }
            (true, ..) => 308.0,
            _ => 60.0,
        };
    }
    if reactions {
        h += 30.0;
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_rows_near_the_viewport_are_shown() {
        // 100 rows of 50 px: 5000 px of content.
        let heights = vec![50.0; 100];
        // At the bottom with a 500 px viewport: rows overlapping 0..1100 from the bottom.
        let v = visible(&heights, 0.0, 500.0);
        assert_eq!(v.range, 78..100);
        assert_eq!(v.below, 0.0);
        assert_eq!(v.above, 78.0 * 50.0);

        // Scrolled 2000 px up: 1400..3100 from the bottom.
        let v = visible(&heights, 2000.0, 500.0);
        assert_eq!(v.range, 38..72);
        assert_eq!(v.below, 28.0 * 50.0);
        assert_eq!(v.above + v.below + 34.0 * 50.0, 5000.0);

        // At the very top everything above is built.
        let v = visible(&heights, 4500.0, 500.0);
        assert_eq!(v.range.start, 0);
        assert_eq!(v.above, 0.0);

        assert_eq!(visible(&[], 0.0, 500.0).range, 0..0);
    }

    #[test]
    fn mixed_heights_add_up() {
        let heights: Vec<f32> = (0..300).map(|i| 20.0 + (i % 7) as f32 * 30.0).collect();
        let total: f32 = heights.iter().sum();
        for from_bottom in [0.0, 333.0, 5000.0, total] {
            let v = visible(&heights, from_bottom, 700.0);
            let shown: f32 = heights[v.range.clone()].iter().sum();
            assert!((v.above + shown + v.below - total).abs() < 0.01);
            assert!(shown >= 700.0_f32.min(total));
        }
    }

    #[test]
    fn heights_report_changes() {
        let mut h = Heights::default();
        assert!(h.set(RowKey::NewMessages, 24.0));
        assert!(!h.set(RowKey::NewMessages, 24.2));
        assert!(h.set(RowKey::NewMessages, 30.0));
        assert_eq!(h.get(&RowKey::NewMessages), Some(30.0));
        h.clear();
        assert_eq!(h.get(&RowKey::NewMessages), None);
    }

    #[test]
    fn estimates_grow_with_content() {
        let short = estimate_message("hi", true, &[], false, false, 600.0, 13.0);
        let long = estimate_message(&"word ".repeat(200), true, &[], false, false, 600.0, 13.0);
        let header = estimate_message("hi", false, &[], false, false, 600.0, 13.0);
        let image = estimate_message(
            "",
            true,
            &[(Some(800), Some(600), true)],
            false,
            false,
            600.0,
            13.0,
        );
        assert!(long > short * 3.0);
        assert!(header > short);
        assert!(image > 300.0 - 1.0);
    }
}
