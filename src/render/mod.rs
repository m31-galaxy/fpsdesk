//! Rendering. A frame is built in three passes:
//!
//! 1. `window`: each window is flattened into its own texture (Smithay, 2D).
//! 2. `scene`: the world is drawn into our framebuffer, one quad per window
//!    (our GL, 3D).
//! 3. `Presenter`: that framebuffer goes to the output as a single element
//!    (Smithay, 2D), which is where screen-space overlays will be added.
//!
//! Nothing here depends on which backend owns the output.

mod gl;
pub mod scene;
pub mod screenshot;
pub mod window;

use std::error::Error;

use glam::Vec2;
use smithay::{
    backend::renderer::{
        Color32F, Renderer,
        damage::OutputDamageTracker,
        element::{Id, Kind, texture::TextureRenderElement},
        gles::{GlesRenderer, GlesTarget, GlesTexture},
    },
    utils::{Physical, Size, Transform},
};

use crate::{
    state::WindowEntry,
    world::{Camera, PIXELS_PER_METRE},
};
use scene::{Quad, SceneRenderer};
use window::WindowTexture;

const SKY: [f32; 3] = [0.05, 0.07, 0.12];

/// Passes 1 and 2: update the window textures, then draw the world as seen by
/// `camera` at `size`. `scale` is how many physical pixels clients draw per
/// logical pixel. Returns the finished image, first row at the bottom.
pub fn render_world(
    renderer: &mut GlesRenderer,
    scene: &mut SceneRenderer,
    windows: &mut [WindowEntry],
    camera: &Camera,
    size: Size<i32, Physical>,
    scale: f64,
) -> Result<GlesTexture, Box<dyn Error>> {
    let mut quads = Vec::with_capacity(windows.len());
    for entry in windows {
        let Some(image) =
            WindowTexture::update(&mut entry.texture, renderer, &entry.window, scale)?
        else {
            continue;
        };
        if image.changed {
            tracing::trace!(slot = entry.slot, "window texture updated");
        }

        entry.panel.size = Vec2::new(image.size.w as f32, image.size.h as f32) / PIXELS_PER_METRE;
        let distance = entry.panel.center.distance_squared(camera.position);
        quads.push((
            distance,
            Quad {
                model: entry.panel.model(),
                texture: image.texture,
            },
        ));
    }

    // Farthest first, so that a translucent panel blends over what is behind it.
    quads.sort_by(|a, b| b.0.total_cmp(&a.0));
    let quads: Vec<Quad> = quads.into_iter().map(|(_, quad)| quad).collect();

    let aspect = size.w as f32 / size.h as f32;
    scene.render(renderer, size, camera.view_projection(aspect), SKY, &quads)
}

/// Pass 3: puts the world image on an output.
pub struct Presenter {
    id: Id,
    damage_tracker: Option<(Size<i32, Physical>, OutputDamageTracker)>,
}

impl Presenter {
    pub fn new() -> Self {
        Self {
            id: Id::new(),
            damage_tracker: None,
        }
    }

    /// Draw `world` over the whole of `framebuffer`, which is `size` pixels
    /// and displayed with `transform`.
    pub fn present(
        &mut self,
        renderer: &mut GlesRenderer,
        framebuffer: &mut GlesTarget<'_>,
        size: Size<i32, Physical>,
        transform: Transform,
        world: GlesTexture,
    ) -> Result<(), Box<dyn Error>> {
        if self.damage_tracker.as_ref().is_none_or(|(s, _)| *s != size) {
            // Scale 1: the world image is already in output pixels.
            self.damage_tracker = Some((size, OutputDamageTracker::new(size, 1.0, transform)));
        }
        let (_, damage_tracker) = self.damage_tracker.as_mut().expect("just created");

        let element = TextureRenderElement::from_static_texture(
            self.id.clone(),
            renderer.context_id(),
            (0.0, 0.0),
            world,
            1,
            // Our framebuffer's first row is the bottom of the picture, where
            // Smithay expects the top.
            Transform::Flipped180,
            None,
            None,
            None,
            None,
            Kind::Unspecified,
        );

        // Age 0: the world changes every frame, so always redraw all of it.
        damage_tracker.render_output(renderer, framebuffer, 0, &[element], Color32F::BLACK)?;
        Ok(())
    }
}
