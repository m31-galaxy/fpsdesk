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
            Id, Kind, render_elements,
            solid::SolidColorRenderElement,
            surface::{WaylandSurfaceRenderElement, render_elements_from_surface_tree},
            texture::TextureRenderElement,
            utils::RescaleRenderElement,
        },
        gles::{GlesRenderer, GlesTarget, GlesTexture},
        utils::CommitCounter,
    },
    input::pointer::CursorImageSurfaceData,
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Physical, Point, Rectangle, Size, Transform},
    wayland::compositor::with_states,
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
const CROSSHAIR_EDGE_COLOR: Color32F = Color32F::new(0.05, 0.05, 0.05, 1.0);

render_elements! {
    /// Everything pass 3 puts on an output.
    OutputElement<=GlesRenderer>;
    World=TextureRenderElement<GlesTexture>,
    Overlay=SolidColorRenderElement,
    Cursor=RescaleRenderElement<WaylandSurfaceRenderElement<GlesRenderer>>,
}

/// What pass 3 draws over the world.
#[derive(Default)]
pub struct Overlays<'a> {
    /// A crosshair in the middle of the view.
    pub crosshair: bool,
    /// A client's cursor image.
    pub cursor: Option<CursorSprite<'a>>,
}

/// A cursor image supplied by a client, and where to draw it.
pub struct CursorSprite<'a> {
    pub surface: &'a WlSurface,
    /// Where the pointer is on the output. The image's hotspot goes here.
    pub position: Point<i32, Physical>,
    /// Output pixels per logical pixel.
    pub scale: f64,
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
        let image = WindowTexture::update(&mut entry.texture, renderer, &entry.window, scale)?;
        entry.extent = image.as_ref().map(|image| image.extent);
        let Some(image) = image else {
            continue;
        };
        if image.changed {
            tracing::trace!(slot = entry.slot, "window texture updated");
        }

        // The panel is the window proper; its picture may be bigger, to take
        // in open popups.
        let geometry = image.extent.geometry.size;
        entry.panel.size = Vec2::new(geometry.w as f32, geometry.h as f32) / PIXELS_PER_METRE;
        let content = entry.content_panel().unwrap_or(entry.panel);

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
                model: content.model(),
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
    crosshair_ids: [Id; 4],
    damage_tracker: Option<(Size<i32, Physical>, OutputDamageTracker)>,
}

impl Presenter {
    pub fn new() -> Self {
        Self {
            world_id: Id::new(),
            crosshair_ids: std::array::from_fn(|_| Id::new()),
            damage_tracker: None,
        }
    }

    /// Draw `world` over the whole of `framebuffer`, which is `size` pixels
    /// and displayed with `transform`, and `overlays` over that.
    pub fn present(
        &mut self,
        renderer: &mut GlesRenderer,
        framebuffer: &mut GlesTarget<'_>,
        size: Size<i32, Physical>,
        transform: Transform,
        world: GlesTexture,
        overlays: Overlays<'_>,
    ) -> Result<(), Box<dyn Error>> {
        if self.damage_tracker.as_ref().is_none_or(|(s, _)| *s != size) {
            // Scale 1: everything here is positioned in output pixels.
            self.damage_tracker = Some((size, OutputDamageTracker::new(size, 1.0, transform)));
        }
        let (_, damage_tracker) = self.damage_tracker.as_mut().expect("just created");

        // Front to back: the first element is drawn on top.
        let mut elements: Vec<OutputElement> = Vec::new();

        if let Some(cursor) = overlays.cursor {
            let hotspot = with_states(cursor.surface, |states| {
                states
                    .data_map
                    .get::<CursorImageSurfaceData>()
                    .map(|data| data.lock().unwrap().hotspot)
                    .unwrap_or_default()
            });
            let corner = cursor.position - hotspot.to_physical_precise_round(cursor.scale);
            let surfaces: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
                render_elements_from_surface_tree(
                    renderer,
                    cursor.surface,
                    corner,
                    1.0,
                    1.0,
                    Kind::Cursor,
                );
            // The surface elements are sized in logical pixels; scale them up
            // to output pixels about the image's corner.
            elements.extend(surfaces.into_iter().map(|surface| {
                OutputElement::Cursor(RescaleRenderElement::from_element(
                    surface,
                    corner,
                    cursor.scale,
                ))
            }));
        }

        if overlays.crosshair {
            let centre = Point::<i32, Physical>::from((size.w / 2, size.h / 2));
            // A horizontal and a vertical bar, each two arms long, in a light
            // colour; then the same again a pixel bigger all round in a dark
            // one, as an edge that keeps the crosshair visible over a white
            // window.
            let (long, thick) = (CROSSHAIR_ARM * 2, CROSSHAIR_THICKNESS);
            let bars: [(Size<i32, Physical>, Color32F); 4] = [
                ((long, thick).into(), CROSSHAIR_COLOR),
                ((thick, long).into(), CROSSHAIR_COLOR),
                ((long + 2, thick + 2).into(), CROSSHAIR_EDGE_COLOR),
                ((thick + 2, long + 2).into(), CROSSHAIR_EDGE_COLOR),
            ];
            for (id, (bar, color)) in self.crosshair_ids.iter().zip(bars) {
                let geometry = Rectangle::new(
                    Point::from((centre.x - bar.w / 2, centre.y - bar.h / 2)),
                    bar,
                );
                elements.push(OutputElement::Overlay(SolidColorRenderElement::new(
                    id.clone(),
                    geometry,
                    CommitCounter::default(),
                    color,
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
