//! Save what was just rendered as a PNG, for checking output without eyes on
//! the window.

use std::{error::Error, fs::File, io::BufWriter, path::Path};

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            ExportMem, Frame, Renderer,
            gles::{GlesRenderer, GlesTarget},
        },
    },
    utils::{Physical, Rectangle, Size, Transform},
};

/// Write the contents of `framebuffer`, which must be a window or output
/// surface (first row at the bottom), to `path`. The framebuffer can still be
/// presented afterwards.
pub fn save(
    renderer: &mut GlesRenderer,
    framebuffer: &mut GlesTarget<'_>,
    size: Size<i32, Physical>,
    path: &Path,
) -> Result<(), Box<dyn Error>> {
    let region = Rectangle::from_size((size.w, size.h).into());
    let mapping = renderer.copy_framebuffer(framebuffer, region, Fourcc::Abgr8888)?;
    let pixels = renderer.map_texture(&mapping)?;

    let row_len = size.w as usize * 4;
    let mut image = Vec::with_capacity(pixels.len());
    for row in pixels.chunks_exact(row_len).rev() {
        image.extend_from_slice(row);
    }
    // The surface's alpha channel is meaningless; make the picture opaque.
    for pixel in image.as_chunks_mut::<4>().0 {
        pixel[3] = 0xff;
    }

    // Mapping the pixels left the GL context current without the surface, and
    // a swap would then fail. Starting a frame makes the surface current
    // again; an empty one draws nothing.
    let _sync = renderer
        .render(framebuffer, size, Transform::Normal)?
        .finish()?;

    let mut encoder = png::Encoder::new(
        BufWriter::new(File::create(path)?),
        size.w as u32,
        size.h as u32,
    );
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(&image)?;
    Ok(())
}
