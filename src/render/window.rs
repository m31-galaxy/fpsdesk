//! Pass 1: flatten each window into a texture of its own.
//!
//! Smithay's 2D pipeline does the compositing, so subsurfaces, buffer scales
//! and transforms, viewport crops and video formats are all its problem. What
//! comes out is one plain RGBA texture per window, which pass 2 can put on a
//! quad and which, unlike a client's own buffer, can have mipmaps.
//!
//! The texture covers the window and any popups (menus, tooltips) it has
//! open, which can hang past the window's edges.

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
    desktop::{PopupManager, Window},
    utils::{Logical, Physical, Point, Rectangle, Scale, Size, Transform},
};

pub struct WindowTexture {
    texture: GlesTexture,
    size: Size<i32, Physical>,
    scale: f64,
    damage_tracker: OutputDamageTracker,
}

/// The area a window occupies, in window coordinates: logical pixels measured
/// from the origin of its surface tree.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Extent {
    /// The window proper, without any shadow the client draws around it.
    pub geometry: Rectangle<i32, Logical>,
    /// The window together with its open popups. This is what the texture
    /// shows.
    pub content: Rectangle<i32, Logical>,
}

impl Extent {
    /// Measure `window`. `None` while it has nothing to show.
    pub fn of(window: &Window) -> Option<Self> {
        let geometry = window.geometry();
        if geometry.size.w <= 0 || geometry.size.h <= 0 {
            return None;
        }

        let mut content = geometry;
        if let Some(toplevel) = window.toplevel() {
            for (popup, location) in PopupManager::popups_for_surface(toplevel.wl_surface()) {
                // A popup's location is that of its own geometry relative to
                // the window's.
                let rect = Rectangle::new(geometry.loc + location, popup.geometry().size);
                content = content.merge(rect);
            }
        }
        Some(Self { geometry, content })
    }
}

impl WindowTexture {
    /// GL name of the texture. Premultiplied alpha, first row at the top.
    pub fn id(&self) -> u32 {
        self.texture.tex_id()
    }
}

/// A window's current picture.
pub struct WindowImage {
    /// GL name of the texture. Premultiplied alpha, first row at the top.
    pub texture: u32,
    /// What the texture covers.
    pub extent: Extent,
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
        let Some(extent) = Extent::of(window) else {
            return Ok(None);
        };
        let size: Size<i32, Physical> = extent.content.size.to_physical_precise_round(scale);

        if slot
            .as_ref()
            .is_none_or(|t| t.size != size || t.scale != scale)
        {
            *slot = Some(Self::new(renderer, size, scale)?);
        }
        let this = slot.as_mut().expect("slot was just filled");

        // Draw the surface tree so that the content's corner lands on the
        // texture's corner.
        let origin: Point<i32, Physical> =
            (Point::default() - extent.content.loc).to_physical_precise_round(scale);
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
            extent,
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
