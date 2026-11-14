//! Rendering. A frame is built in three passes:
//!
//! 1. `window`: each window is flattened into its own texture (Smithay, 2D).
//! 2. `scene`: the world is drawn into our framebuffer, one quad per window
//!    (our GL, 3D).
//! 3. `Presenter`: that framebuffer goes to the output as a single element
//!    with screen-space overlays such as the crosshair on top (Smithay, 2D).
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
        element::{
            Id, Kind, render_elements, solid::SolidColorRenderElement,
            texture::TextureRenderElement,
        },
        gles::{GlesRenderer, GlesTarget, GlesTexture},
        utils::CommitCounter,
    },
    utils::{Physical, Point, Rectangle, Size, Transform},
};

use crate::{
    state::{Highlight, WindowEntry},
    world::{Camera, PIXELS_PER_METRE, Panel},
};
use scene::{Quad, SceneRenderer};
use window::WindowTexture;

/// Width of the outline around a highlighted window, in metres.
const OUTLINE_WIDTH: f32 = 0.012;
const HOVERED_COLOR: [f32; 3] = [0.35, 0.75, 1.0];
const FOCUSED_COLOR: [f32; 3] = [1.0, 0.7, 0.2];

/// Length of each arm of the crosshair, and their thickness, in pixels.
const CROSSHAIR_ARM: i32 = 9;
const CROSSHAIR_THICKNESS: i32 = 2;
const CROSSHAIR_COLOR: Color32F = Color32F::new(0.95, 0.95, 0.95, 1.0);

render_elements! {
    /// Everything pass 3 puts on an output.
    OutputElement<=GlesRenderer>;
    World=TextureRenderElement<GlesTexture>,
    Overlay=SolidColorRenderElement,
}

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

        let outline = match entry.highlight {
            Highlight::None => None,
            Highlight::Hovered => Some(HOVERED_COLOR),
            Highlight::Focused => Some(FOCUSED_COLOR),
        }
        .map(|color| {
            let border = Panel {
                size: entry.panel.size + 2.0 * OUTLINE_WIDTH,
                ..entry.panel
            };
            (border.model(), color)
        });

        let distance = entry.panel.center.distance_squared(camera.position);
        quads.push((
            distance,
            Quad {
                model: entry.panel.model(),
                texture: image.texture,
                outline,
            },
        ));
    }

    // Farthest first, so that a translucent panel blends over what is behind it.
    quads.sort_by(|a, b| b.0.total_cmp(&a.0));
    let quads: Vec<Quad> = quads.into_iter().map(|(_, quad)| quad).collect();

    scene.render(renderer, size, camera, &quads)
}

/// Pass 3: puts the world image, and anything drawn over it, on an output.
pub struct Presenter {
    world_id: Id,
    crosshair_ids: [Id; 2],
    damage_tracker: Option<(Size<i32, Physical>, OutputDamageTracker)>,
}

impl Presenter {
    pub fn new() -> Self {
        Self {
            world_id: Id::new(),
            crosshair_ids: [Id::new(), Id::new()],
            damage_tracker: None,
        }
    }

    /// Draw `world` over the whole of `framebuffer`, which is `size` pixels
    /// and displayed with `transform`, with a crosshair in the middle if
    /// `crosshair` is set.
    pub fn present(
        &mut self,
        renderer: &mut GlesRenderer,
        framebuffer: &mut GlesTarget<'_>,
        size: Size<i32, Physical>,
        transform: Transform,
        world: GlesTexture,
        crosshair: bool,
    ) -> Result<(), Box<dyn Error>> {
        if self.damage_tracker.as_ref().is_none_or(|(s, _)| *s != size) {
            // Scale 1: the world image is already in output pixels.
            self.damage_tracker = Some((size, OutputDamageTracker::new(size, 1.0, transform)));
        }
        let (_, damage_tracker) = self.damage_tracker.as_mut().expect("just created");

        // Front to back: the first element is drawn on top.
        let mut elements: Vec<OutputElement> = Vec::with_capacity(3);
        if crosshair {
            let centre = Point::<i32, Physical>::from((size.w / 2, size.h / 2));
            // A horizontal and a vertical bar, each two arms long.
            let bars: [Size<i32, Physical>; 2] = [
                (CROSSHAIR_ARM * 2, CROSSHAIR_THICKNESS).into(),
                (CROSSHAIR_THICKNESS, CROSSHAIR_ARM * 2).into(),
            ];
            for (id, bar) in self.crosshair_ids.iter().zip(bars) {
                let geometry = Rectangle::new(
                    Point::from((centre.x - bar.w / 2, centre.y - bar.h / 2)),
                    bar,
                );
                elements.push(OutputElement::Overlay(SolidColorRenderElement::new(
                    id.clone(),
                    geometry,
                    CommitCounter::default(),
                    CROSSHAIR_COLOR,
                    Kind::Unspecified,
                )));
            }
        }
        elements.push(OutputElement::World(
            TextureRenderElement::from_static_texture(
                self.world_id.clone(),
                renderer.context_id(),
                (0.0, 0.0),
                world,
                1,
                // Our framebuffer's first row is the bottom of the picture,
                // where Smithay expects the top.
                Transform::Flipped180,
                None,
                None,
                None,
                None,
                Kind::Unspecified,
            ),
        ));

        // Age 0: the world changes every frame, so always redraw all of it.
        damage_tracker.render_output(renderer, framebuffer, 0, &elements, Color32F::BLACK)?;
        Ok(())
    }
}
