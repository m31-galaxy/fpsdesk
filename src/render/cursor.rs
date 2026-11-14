//! The built-in mouse cursor: a plain arrow, for backends with no host cursor
//! to show (the TTY backend). Clients' own cursor images are drawn elsewhere.

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            element::{
                Kind,
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
            },
            gles::{GlesError, GlesRenderer},
        },
    },
    utils::{Logical, Physical, Point, Rectangle, Size, Transform},
};

/// `X` is the outline, `.` the fill, and a space is transparent. The tip, at
/// the top left, is the hotspot.
const ARROW: [&str; 19] = [
    "X           ",
    "XX          ",
    "X.X         ",
    "X..X        ",
    "X...X       ",
    "X....X      ",
    "X.....X     ",
    "X......X    ",
    "X.......X   ",
    "X........X  ",
    "X.....XXXXX ",
    "X..X..X     ",
    "X.X X..X    ",
    "XX  X..X    ",
    "X    X..X   ",
    "     X..X   ",
    "      X..X  ",
    "      X..X  ",
    "       XX   ",
];

/// Where to draw the built-in cursor.
#[derive(Debug, Clone, Copy)]
pub struct BuiltinCursor {
    /// Where the pointer is on the output; the arrow's tip goes here.
    pub position: Point<i32, Physical>,
    /// Output pixels per logical pixel.
    pub scale: f64,
}

/// The arrow as a buffer to draw. Cheap to clone.
pub fn arrow_buffer() -> MemoryRenderBuffer {
    let width = ARROW[0].len() as i32;
    let mut pixels = Vec::with_capacity(ARROW.len() * ARROW[0].len() * 4);
    for row in ARROW {
        debug_assert_eq!(row.len() as i32, width);
        for cell in row.bytes() {
            // Argb8888 is little-endian: bytes are B, G, R, A, premultiplied.
            pixels.extend_from_slice(match cell {
                b'X' => &[0, 0, 0, 255],
                b'.' => &[255, 255, 255, 255],
                _ => &[0, 0, 0, 0],
            });
        }
    }
    MemoryRenderBuffer::from_slice(
        &pixels,
        Fourcc::Argb8888,
        (width, ARROW.len() as i32),
        1,
        Transform::Normal,
        None,
    )
}

/// The element that draws `buffer` (from `arrow_buffer`) as `cursor` asks.
pub fn arrow_element(
    renderer: &mut GlesRenderer,
    buffer: &MemoryRenderBuffer,
    cursor: BuiltinCursor,
) -> Result<MemoryRenderBufferRenderElement<GlesRenderer>, GlesError> {
    let image = Size::<i32, Logical>::from((ARROW[0].len() as i32, ARROW.len() as i32));
    // Scale the arrow up with the output so that it stays about as big to the
    // eye, in whole pixels so that it stays crisp.
    let zoom = cursor.scale.round().max(1.0) as i32;
    MemoryRenderBufferRenderElement::from_buffer(
        renderer,
        cursor.position.to_f64(),
        buffer,
        None,
        Some(Rectangle::from_size(image.to_f64())),
        Some((image.w * zoom, image.h * zoom).into()),
        Kind::Cursor,
    )
}
