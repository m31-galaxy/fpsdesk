//! Text as pixels: an 8x8 bitmap font drawn into an RGBA buffer.
//!
//! No GL or Smithay types here, so the drawing can be tested and reused for any
//! on-screen text. The renderer uploads a `Bitmap` as a texture.

use font8x8::{BASIC_FONTS, UnicodeFonts};

/// Side of a glyph cell in font pixels.
pub const GLYPH_SIZE: u32 = 8;

/// A colour as red, green, blue, alpha bytes.
pub type Rgba = [u8; 4];

/// A rectangle of RGBA pixels, row by row from the top left.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    /// Four bytes per pixel in the order red, green, blue, alpha.
    pub pixels: Vec<u8>,
}

impl Bitmap {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            pixels: vec![0; width as usize * height as usize * 4],
        }
    }

    /// The pixel at `x`, `y`, or `None` outside the bitmap.
    #[cfg(test)]
    pub fn pixel(&self, x: u32, y: u32) -> Option<Rgba> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let at = (y as usize * self.width as usize + x as usize) * 4;
        self.pixels[at..at + 4].try_into().ok()
    }

    /// Fill the rectangle at `x`, `y` of `w` by `h` pixels, clipped to the bitmap.
    pub fn fill_rect(&mut self, x: u32, y: u32, w: u32, h: u32, color: Rgba) {
        for row in y..y.saturating_add(h).min(self.height) {
            for col in x..x.saturating_add(w).min(self.width) {
                let at = (row as usize * self.width as usize + col as usize) * 4;
                self.pixels[at..at + 4].copy_from_slice(&color);
            }
        }
    }

    /// Draw `ch` with its top left corner at `x`, `y`, each font pixel being a
    /// `scale` by `scale` block. Characters the font lacks draw as `?`.
    pub fn draw_char(&mut self, x: u32, y: u32, ch: char, scale: u32, color: Rgba) {
        let glyph = BASIC_FONTS
            .get(ch)
            .or_else(|| BASIC_FONTS.get('?'))
            .unwrap_or([0; 8]);
        for (row, bits) in glyph.iter().enumerate() {
            for col in 0..GLYPH_SIZE {
                // The least significant bit is the leftmost pixel.
                if bits >> col & 1 != 0 {
                    self.fill_rect(x + col * scale, y + row as u32 * scale, scale, scale, color);
                }
            }
        }
    }

    /// Draw `text` on one line from `x`, `y`. Returns the x just past its end.
    pub fn draw_text(&mut self, x: u32, y: u32, text: &str, scale: u32, color: Rgba) -> u32 {
        let mut x = x;
        for ch in text.chars() {
            self.draw_char(x, y, ch, scale, color);
            x += text_width(1, scale);
        }
        x
    }
}

/// How wide `chars` characters are at `scale`.
pub fn text_width(chars: u32, scale: u32) -> u32 {
    chars * GLYPH_SIZE * scale
}

#[cfg(test)]
mod tests {
    use super::*;

    const WHITE: Rgba = [255; 4];

    fn lit(bitmap: &Bitmap) -> usize {
        bitmap.pixels.chunks(4).filter(|p| p[3] != 0).count()
    }

    #[test]
    fn a_new_bitmap_is_transparent() {
        let bitmap = Bitmap::new(3, 2);
        assert_eq!(bitmap.pixels.len(), 3 * 2 * 4);
        assert_eq!(lit(&bitmap), 0);
    }

    #[test]
    fn fill_rect_is_clipped() {
        let mut bitmap = Bitmap::new(4, 4);
        bitmap.fill_rect(2, 2, 10, 10, WHITE);
        assert_eq!(lit(&bitmap), 4);
        assert_eq!(bitmap.pixel(3, 3), Some(WHITE));
        assert_eq!(bitmap.pixel(1, 1), Some([0; 4]));
        assert_eq!(bitmap.pixel(4, 0), None);
    }

    #[test]
    fn a_glyph_draws_inside_its_cell() {
        let mut bitmap = Bitmap::new(16, 8);
        bitmap.draw_char(8, 0, 'A', 1, WHITE);
        assert!(lit(&bitmap) > 0);
        // Nothing in the first cell.
        for x in 0..8 {
            for y in 0..8 {
                assert_eq!(bitmap.pixel(x, y), Some([0; 4]));
            }
        }
    }

    #[test]
    fn scaling_multiplies_the_area() {
        let mut one = Bitmap::new(8, 8);
        one.draw_char(0, 0, 'g', 1, WHITE);
        let mut three = Bitmap::new(24, 24);
        three.draw_char(0, 0, 'g', 3, WHITE);
        assert_eq!(lit(&three), lit(&one) * 9);
    }

    #[test]
    fn a_space_draws_nothing_and_an_unknown_char_draws_something() {
        let mut bitmap = Bitmap::new(8, 8);
        bitmap.draw_char(0, 0, ' ', 1, WHITE);
        assert_eq!(lit(&bitmap), 0);
        bitmap.draw_char(0, 0, '\u{1F600}', 1, WHITE);
        assert!(lit(&bitmap) > 0);
    }

    #[test]
    fn draw_text_advances_one_cell_a_character() {
        let mut bitmap = Bitmap::new(64, 16);
        assert_eq!(bitmap.draw_text(0, 0, "abc", 2, WHITE), text_width(3, 2));
    }

    #[test]
    fn drawing_past_the_edge_does_not_panic() {
        let mut bitmap = Bitmap::new(5, 5);
        bitmap.draw_text(3, 3, "hello", 2, WHITE);
    }
}
