//! The mouse pointer in a shared screen. Desktop Duplication leaves the pointer out of the
//! picture and hands over its shape separately, in one of three formats. [`Shape::convert`] turns
//! each into two pictures that the GPU draws over the screen (`dxgi::CursorDrawer`):
//! - `color`: premultiplied BGRA, blended normally;
//! - `invert`: white where the pointer inverts the screen beneath it (the text cursor, for one),
//!   drawn with an inverting blend; `None` when no pixel inverts.

/// `DXGI_OUTDUPL_POINTER_SHAPE_TYPE_*`.
pub const MONOCHROME: u32 = 1;
pub const COLOR: u32 = 2;
pub const MASKED_COLOR: u32 = 4;

/// A pointer shape, ready to upload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shape {
    pub width: u32,
    pub height: u32,
    /// Premultiplied BGRA, `width * height * 4` bytes.
    pub color: Vec<u8>,
    /// BGRA: white where the screen is inverted, black (no change) elsewhere.
    pub invert: Option<Vec<u8>>,
}

impl Shape {
    /// Converts a shape from `GetFramePointerShape`: `kind` is its type, `width` and `height`
    /// as reported (a monochrome shape's height covers both its masks), `pitch` the bytes per
    /// row of `data`. `None` if the data is too short or the type unknown.
    pub fn convert(kind: u32, width: u32, height: u32, pitch: u32, data: &[u8]) -> Option<Shape> {
        let (w, pitch) = (width as usize, pitch as usize);
        match kind {
            MONOCHROME => {
                // An AND mask on top of an XOR mask, one bit per pixel, high bit first.
                let h = height as usize / 2;
                if w == 0 || h == 0 || data.len() < pitch * h * 2 || pitch * 8 < w {
                    return None;
                }
                let bit = |row: usize, x: usize| data[row * pitch + x / 8] & (0x80 >> (x % 8)) != 0;
                let mut color = vec![0u8; w * h * 4];
                let mut invert = vec![0u8; w * h * 4];
                let mut inverts = false;
                for y in 0..h {
                    for x in 0..w {
                        let i = (y * w + x) * 4;
                        match (bit(y, x), bit(y + h, x)) {
                            // Keep the screen.
                            (true, false) => {}
                            // Invert the screen.
                            (true, true) => {
                                invert[i..i + 4].copy_from_slice(&[255, 255, 255, 255]);
                                inverts = true;
                            }
                            // Black or white.
                            (false, xor) => {
                                let v = if xor { 255 } else { 0 };
                                color[i..i + 4].copy_from_slice(&[v, v, v, 255]);
                            }
                        }
                    }
                }
                Some(Shape {
                    width,
                    height: h as u32,
                    color,
                    invert: inverts.then_some(invert),
                })
            }
            COLOR | MASKED_COLOR => {
                let h = height as usize;
                if w == 0 || h == 0 || data.len() < pitch * (h - 1) + w * 4 || pitch < w * 4 {
                    return None;
                }
                let mut color = vec![0u8; w * h * 4];
                let mut invert = vec![0u8; w * h * 4];
                let mut inverts = false;
                for y in 0..h {
                    for x in 0..w {
                        let s = y * pitch + x * 4;
                        let [b, g, r, a] = [data[s], data[s + 1], data[s + 2], data[s + 3]];
                        let i = (y * w + x) * 4;
                        if kind == COLOR {
                            // Straight alpha: premultiply.
                            let p = |c: u8| ((u16::from(c) * u16::from(a) + 127) / 255) as u8;
                            color[i..i + 4].copy_from_slice(&[p(b), p(g), p(r), a]);
                        } else if a == 0 {
                            // The mask bit clear: the color replaces the screen.
                            color[i..i + 4].copy_from_slice(&[b, g, r, 255]);
                        } else if (b, g, r) != (0, 0, 0) {
                            // The mask bit set: the color is XORed with the screen. Drawn as
                            // an inversion by the color (exact for black and white).
                            invert[i..i + 4].copy_from_slice(&[b, g, r, 255]);
                            inverts = true;
                        }
                    }
                }
                Some(Shape {
                    width,
                    height: h as u32,
                    color,
                    invert: inverts.then_some(invert),
                })
            }
            _ => None,
        }
    }
}

/// Where the pointer is and what it looks like, kept across frames (Desktop Duplication only
/// reports changes).
#[derive(Debug, Default)]
pub struct Pointer {
    pub visible: bool,
    /// The top left corner of the shape, in the screen's pixels.
    pub position: (i32, i32),
    pub shape: Option<Shape>,
    /// The shape changed since it was last uploaded.
    pub shape_changed: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn px(shape: &Shape, x: usize, y: usize) -> [u8; 4] {
        let i = (y * shape.width as usize + x) * 4;
        shape.color[i..i + 4].try_into().unwrap()
    }

    fn inv(shape: &Shape, x: usize, y: usize) -> [u8; 4] {
        let i = (y * shape.width as usize + x) * 4;
        shape.invert.as_ref().unwrap()[i..i + 4].try_into().unwrap()
    }

    #[test]
    fn monochrome_masks() {
        // 4×1 pixels, pitch 1 byte: AND row then XOR row. Pixels: keep, invert, black, white.
        let and = 0b1100_0000u8;
        let xor = 0b0101_0000u8;
        let shape = Shape::convert(MONOCHROME, 4, 2, 1, &[and, xor]).unwrap();
        assert_eq!((shape.width, shape.height), (4, 1));
        assert_eq!(px(&shape, 0, 0), [0, 0, 0, 0]);
        assert_eq!(px(&shape, 1, 0), [0, 0, 0, 0]);
        assert_eq!(inv(&shape, 1, 0), [255; 4]);
        assert_eq!(px(&shape, 2, 0), [0, 0, 0, 255]);
        assert_eq!(px(&shape, 3, 0), [255, 255, 255, 255]);
        assert_eq!(inv(&shape, 0, 0), [0; 4]);
    }

    #[test]
    fn monochrome_without_inversion_has_no_invert_picture() {
        let shape = Shape::convert(MONOCHROME, 8, 2, 1, &[0x00, 0xff]).unwrap();
        assert!(shape.invert.is_none());
        assert_eq!(px(&shape, 7, 0), [255, 255, 255, 255]);
    }

    #[test]
    fn color_is_premultiplied() {
        // One pixel with padding in the pitch: half-transparent white.
        let shape = Shape::convert(COLOR, 1, 1, 8, &[255, 255, 255, 128, 9, 9, 9, 9]).unwrap();
        assert_eq!(px(&shape, 0, 0), [128, 128, 128, 128]);
        assert!(shape.invert.is_none());
    }

    #[test]
    fn masked_color_replaces_or_inverts() {
        // Replace with red, XOR with white, XOR with black (no change).
        let data = [0, 0, 255, 0, 255, 255, 255, 255, 0, 0, 0, 255];
        let shape = Shape::convert(MASKED_COLOR, 3, 1, 12, &data).unwrap();
        assert_eq!(px(&shape, 0, 0), [0, 0, 255, 255]);
        assert_eq!(px(&shape, 1, 0), [0, 0, 0, 0]);
        assert_eq!(inv(&shape, 1, 0), [255, 255, 255, 255]);
        assert_eq!(px(&shape, 2, 0), [0, 0, 0, 0]);
        assert_eq!(inv(&shape, 2, 0), [0; 4]);
    }

    #[test]
    fn short_or_unknown_data_is_refused() {
        assert!(Shape::convert(MONOCHROME, 8, 2, 1, &[0]).is_none());
        assert!(Shape::convert(COLOR, 2, 2, 8, &[0; 12]).is_none());
        assert!(Shape::convert(7, 1, 1, 4, &[0; 4]).is_none());
        assert!(Shape::convert(COLOR, 0, 0, 0, &[]).is_none());
    }
}
