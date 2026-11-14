//! The weapon at the bottom of the screen, in chunky pixel art made of solid
//! rectangles, with a muzzle flash over it.

use smithay::{
    backend::renderer::{
        Color32F,
        element::{Id, Kind, solid::SolidColorRenderElement},
        utils::CommitCounter,
    },
    utils::{Physical, Point, Rectangle, Size},
};

/// The weapon's pixel art, nose up. `.` is clear; see `colour` for the rest.
const WEAPON: [&str; 20] = [
    "......KKKK......",
    ".....KllggK.....",
    ".....KlgddK.....",
    ".....KlgddK.....",
    ".....KlgddK.....",
    ".....KlgddK.....",
    "....KKlgddKK....",
    "...KllllggddK...",
    "..KllggggggggdK.",
    "..KlgggggggggddK",
    "..KKKKKKKKKKKKKK",
    "....KbbbbbBBBK..",
    "....KbbbbbBBBK..",
    "....KbbbbbBBBK..",
    "....KbbbbbBBBK..",
    "....KbbbbbBBBK..",
    "....KbbbbbBBBK..",
    "....KbbbbbBBBK..",
    "....KbbbbbBBBK..",
    "....KKKKKKKKKK..",
];

const FLASH: [&str; 9] = [
    ".....y.....",
    "..y..Y..y..",
    "...y.Y.y...",
    "....yWy....",
    ".yYYWWWYYy.",
    "yYWWWWWWWYy",
    ".yYYWWWYYy.",
    "....yWy....",
    "...y.Y.y...",
];

fn colour(c: char) -> Option<Color32F> {
    let rgb = match c {
        'K' => [0.04, 0.04, 0.05],
        'l' => [0.80, 0.82, 0.86],
        'g' => [0.55, 0.58, 0.64],
        'd' => [0.30, 0.32, 0.38],
        'b' => [0.62, 0.38, 0.18],
        'B' => [0.42, 0.24, 0.10],
        'y' => [1.00, 0.55, 0.10],
        'Y' => [1.00, 0.90, 0.30],
        'W' => [1.00, 1.00, 0.90],
        _ => return None,
    };
    Some(Color32F::new(rgb[0], rgb[1], rgb[2], 1.0))
}

/// What the weapon is doing this frame.
#[derive(Debug, Clone, Copy)]
pub struct WeaponView {
    /// How far it is thrown back by the last shot: 1 to 0.
    pub kick: f32,
    /// Muzzle flash: 1 to 0.
    pub flash: f32,
}

/// Turns the weapon into render elements, keeping their ids between frames.
pub struct WeaponPainter {
    ids: Vec<Id>,
}

impl WeaponPainter {
    pub fn new() -> Self {
        Self { ids: Vec::new() }
    }

    /// The elements for a view of `size` pixels, topmost first.
    pub fn elements(
        &mut self,
        size: Size<i32, Physical>,
        view: WeaponView,
    ) -> Vec<SolidColorRenderElement> {
        // Each art pixel is a block this many screen pixels across.
        let block = (size.h / 90).max(2);
        let width = WEAPON[0].len() as i32 * block;
        let height = WEAPON.len() as i32 * block;
        // Centred, sunk a little below the screen's edge, and jumping up with
        // the kick.
        let left = (size.w - width) / 2;
        let top = size.h - height + 2 * block - (view.kick * 5.0 * block as f32) as i32;

        let mut rects: Vec<(Rectangle<i32, Physical>, Color32F)> = Vec::new();

        if view.flash > 0.0 {
            let flash_block = (block as f32 * (0.7 + 0.6 * view.flash)).round() as i32;
            let flash_width = FLASH[0].len() as i32 * flash_block;
            let flash_height = FLASH.len() as i32 * flash_block;
            // Centred on the barrel's tip.
            paint(
                &FLASH,
                flash_block,
                (size.w - flash_width) / 2,
                top - flash_height / 2,
                &mut rects,
            );
        }
        paint(&WEAPON, block, left, top, &mut rects);

        while self.ids.len() < rects.len() {
            self.ids.push(Id::new());
        }
        rects
            .into_iter()
            .zip(&self.ids)
            .map(|((geometry, color), id)| {
                SolidColorRenderElement::new(
                    id.clone(),
                    geometry,
                    CommitCounter::default(),
                    color,
                    Kind::Unspecified,
                )
            })
            .collect()
    }
}

/// Add a rectangle for each horizontal run of one colour in `art`, whose top
/// left is at (`left`, `top`), at `block` pixels per art pixel.
fn paint(
    art: &[&str],
    block: i32,
    left: i32,
    top: i32,
    rects: &mut Vec<(Rectangle<i32, Physical>, Color32F)>,
) {
    for (row, line) in art.iter().enumerate() {
        let cells: Vec<char> = line.chars().collect();
        let mut x = 0;
        while x < cells.len() {
            let run = cells[x..].iter().take_while(|&&c| c == cells[x]).count();
            if let Some(color) = colour(cells[x]) {
                rects.push((
                    Rectangle::new(
                        Point::from((left + x as i32 * block, top + row as i32 * block)),
                        (run as i32 * block, block).into(),
                    ),
                    color,
                ));
            }
            x += run;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_art_is_rectangular_and_uses_known_colours() {
        for art in [&WEAPON[..], &FLASH[..]] {
            let width = art[0].len();
            for line in art {
                assert_eq!(line.len(), width, "ragged row {line:?}");
                assert!(line.chars().all(|c| c == '.' || colour(c).is_some()));
            }
        }
    }
}
