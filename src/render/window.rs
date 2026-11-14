//! Pass 1: flatten each window into a texture of its own.
//!
//! Smithay's 2D pipeline does the compositing, so subsurfaces, buffer scales
//! and transforms, viewport crops and video formats are all its problem. What
//! comes out is one plain RGBA texture per window, which pass 2 can put on a
//! quad and which, unlike a client's own buffer, can have mipmaps.

use std::error::Error;

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            Bind, Color32F, Offscreen,
            damage::OutputDamageTracker,
            element::{AsRenderElements, surface::WaylandSurfaceRenderElement},
            gles::{GlesRenderer, GlesTexture, ffi},
        },
    },
    desktop::Window,
    utils::{Logical, Physical, Point, Scale, Size, Transform},
};

pub struct WindowTexture {
    texture: GlesTexture,
    size: Size<i32, Physical>,
    scale: f64,
    damage_tracker: OutputDamageTracker,
}

/// A window's current picture.
pub struct WindowImage {
    /// GL name of the texture. Premultiplied alpha, first row at the top.
    pub texture: u32,
    /// Size of the window in logical pixels.
    pub size: Size<i32, Logical>,
    /// Whether the texture's contents changed in this update.
    pub changed: bool,
}

impl WindowTexture {
    /// Bring `slot`'s texture up to date with what `window`'s client last
    /// committed, creating or resizing it as needed. Only damaged areas are
    /// redrawn. Returns `None` while the window has nothing to show.
    pub fn update(
        slot: &mut Option<WindowTexture>,
        renderer: &mut GlesRenderer,
        window: &Window,
        scale: f64,
    ) -> Result<Option<WindowImage>, Box<dyn Error>> {
        // The geometry is the window proper, without any shadow the client
        // draws around it.
        let geometry = window.geometry();
        let size: Size<i32, Physical> = geometry.size.to_physical_precise_round(scale);
        if size.w <= 0 || size.h <= 0 {
            return Ok(None);
        }

        if slot
            .as_ref()
            .is_none_or(|t| t.size != size || t.scale != scale)
        {
            *slot = Some(Self::new(renderer, size, scale)?);
        }
        let this = slot.as_mut().expect("slot was just filled");

        // Draw the surface tree so that the geometry's corner lands on the
        // texture's corner.
        let origin: Point<i32, Physical> =
            (Point::default() - geometry.loc).to_physical_precise_round(scale);
        let elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
            window.render_elements(renderer, origin, Scale::from(scale), 1.0);

        let changed = {
            let mut framebuffer = renderer.bind(&mut this.texture)?;
            // Age 1: the texture still holds exactly what we drew last time.
            let result = this.damage_tracker.render_output(
                renderer,
                &mut framebuffer,
                1,
                &elements,
                Color32F::TRANSPARENT,
            )?;
            result.damage.is_some()
        };

        let texture = this.texture.tex_id();
        if changed {
            renderer.with_context(|gl| unsafe {
                gl.BindTexture(ffi::TEXTURE_2D, texture);
                gl.GenerateMipmap(ffi::TEXTURE_2D);
                gl.BindTexture(ffi::TEXTURE_2D, 0);
            })?;
        }

        Ok(Some(WindowImage {
            texture,
            size: geometry.size,
            changed,
        }))
    }

    fn new(
        renderer: &mut GlesRenderer,
        size: Size<i32, Physical>,
        scale: f64,
    ) -> Result<Self, Box<dyn Error>> {
        let texture: GlesTexture =
            renderer.create_buffer(Fourcc::Abgr8888, (size.w, size.h).into())?;

        let id = texture.tex_id();
        renderer.with_context(|gl| unsafe {
            gl.BindTexture(ffi::TEXTURE_2D, id);
            for (parameter, value) in [
                // Trilinear: without mipmaps, text shimmers badly once a
                // panel is far away or seen at an angle.
                (ffi::TEXTURE_MIN_FILTER, ffi::LINEAR_MIPMAP_LINEAR),
                (ffi::TEXTURE_MAG_FILTER, ffi::LINEAR),
                (ffi::TEXTURE_WRAP_S, ffi::CLAMP_TO_EDGE),
                (ffi::TEXTURE_WRAP_T, ffi::CLAMP_TO_EDGE),
            ] {
                gl.TexParameteri(ffi::TEXTURE_2D, parameter, value as i32);
            }
            gl.BindTexture(ffi::TEXTURE_2D, 0);
        })?;

        Ok(Self {
            texture,
            size,
            scale,
            damage_tracker: OutputDamageTracker::new(size, scale, Transform::Normal),
        })
    }
}
