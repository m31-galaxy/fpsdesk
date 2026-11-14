//! The work of a frame that does not depend on which backend owns the output.
//!
//! A backend's redraw is, in order:
//!
//! 1. `Frame::begin`, for the time step.
//! 2. `prepare`: scripted input, then `advance` and the world passes. The
//!    backend can do its own input handling in between (`before_advance`).
//! 3. Bind its render target and call `Frame::present` on it, which draws
//!    pass 3 and takes the screenshot a `--smoke` run asks for.
//! 4. Put the result on screen however it does that.
//! 5. `finish`: frame callbacks, housekeeping, and flushing clients.
//!
//! The pieces are separate functions rather than one, so that a backend can
//! hold its render target (which borrows its renderer, which lives in `State`)
//! between steps 2 and 5.

use std::{
    error::Error,
    path::PathBuf,
    time::{Duration, Instant},
};

use smithay::{
    backend::renderer::{
        gles::{GlesRenderer, GlesTarget, GlesTexture},
        sync::SyncPoint,
    },
    desktop::utils::send_frames_surface_tree,
    output::Output,
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Physical, Size, Transform},
};
use tracing::info;

use crate::{
    frame_stats::FrameStats,
    input::controls::Mode as ControlMode,
    prompt::Prompt,
    render::{self, BuiltinCursor, CursorSprite, Overlays, Presenter, PromptSprite, screenshot},
    state::State,
};

/// The longest step the world takes in one frame. After a stall (the window
/// was hidden, the session was switched away) this stops a held key from
/// teleporting the viewer.
const MAX_FRAME_STEP: Duration = Duration::from_millis(100);

/// What a backend keeps between frames.
pub struct Frame {
    presenter: Presenter,
    stats: FrameStats,
    last_frame: Instant,
}

/// When a frame began, and how long since the one before.
pub struct FrameStart {
    start: Instant,
    dt: Duration,
}

/// How a frame is to be drawn on its output.
#[derive(Clone, Copy)]
pub struct FrameParams {
    pub size: Size<i32, Physical>,
    /// Physical pixels per logical pixel.
    pub scale: f64,
    /// The output transform: `Flipped180` for a window surface, `Normal` for a
    /// buffer that goes to scanout. See `Presenter::present`.
    pub transform: Transform,
    /// Whether to draw our own mouse cursor, rather than the host showing one.
    pub builtin_cursor: bool,
}

/// A frame whose world is drawn, waiting to be presented.
pub struct Prepared {
    params: FrameParams,
    world: GlesTexture,
    crosshair: bool,
    cursor_surface: Option<WlSurface>,
    cursor_position: (i32, i32),
    builtin_cursor: bool,
    /// The launcher prompt as it stood, if it is open.
    prompt: Option<Prompt>,
    /// Where to save a screenshot of this frame, if this is the frame to take it.
    screenshot: Option<PathBuf>,
}

impl Frame {
    pub fn new() -> Self {
        Self {
            presenter: Presenter::new(),
            stats: FrameStats::new(),
            last_frame: Instant::now(),
        }
    }

    pub fn begin(&mut self) -> FrameStart {
        let start = Instant::now();
        let dt = start.duration_since(self.last_frame).min(MAX_FRAME_STEP);
        self.last_frame = start;
        FrameStart { start, dt }
    }

    /// Pass 3 into `framebuffer`, then the screenshot if `prepared` is the
    /// frame for it. Returns the sync point for the drawing, for backends
    /// that hand the buffer to the display without waiting for the GPU.
    pub fn present(
        &mut self,
        prepared: &Prepared,
        renderer: &mut GlesRenderer,
        framebuffer: &mut GlesTarget<'_>,
    ) -> Result<SyncPoint, Box<dyn Error>> {
        let params = prepared.params;
        let overlays = Overlays {
            prompt: prepared.prompt.as_ref().map(|prompt| PromptSprite {
                prompt,
                scale: params.scale,
            }),
            crosshair: prepared.crosshair,
            cursor: prepared
                .cursor_surface
                .as_ref()
                .map(|surface| CursorSprite {
                    surface,
                    position: prepared.cursor_position.into(),
                    scale: params.scale,
                }),
            builtin_cursor: prepared.builtin_cursor.then_some(BuiltinCursor {
                position: prepared.cursor_position.into(),
                scale: params.scale,
            }),
        };
        let sync = self.presenter.present(
            renderer,
            framebuffer,
            params.size,
            params.transform,
            prepared.world.clone(),
            overlays,
        )?;

        if let Some(path) = &prepared.screenshot {
            screenshot::save(renderer, framebuffer, params.size, params.transform, path)?;
            info!(?path, "saved screenshot");
        }
        Ok(sync)
    }

    /// Note how long the frame took, for the frame time log.
    pub fn record(&mut self, start: &FrameStart) {
        self.stats.frame(start.start.elapsed());
    }
}

/// Run scripted input, advance the world, and draw it (passes 1 and 2).
/// `before_advance` is for input the backend has to feed in itself.
pub fn prepare(
    state: &mut State,
    start: &FrameStart,
    params: FrameParams,
    before_advance: impl FnOnce(&mut State),
) -> Result<Prepared, Box<dyn Error>> {
    state.run_script_step();
    before_advance(state);
    state.advance(start.dt.as_secs_f32());

    let world = render::render_world(
        state.backend.renderer(),
        &mut state.scene,
        &mut state.windows,
        &state.camera,
        params.size,
        params.scale,
    )?;

    let smoke_finished = state.smoke_frame_rendered();
    let screenshot = state
        .smoke
        .as_ref()
        .and_then(|smoke| smoke.screenshot.clone())
        .filter(|_| smoke_finished);

    let cursor = (state.cursor.as_dvec2() * params.scale).round().as_ivec2();
    Ok(Prepared {
        params,
        world,
        crosshair: state.mode == ControlMode::Walk,
        // Cloned so that it does not hold a borrow of `state`.
        cursor_surface: state.cursor_surface().cloned(),
        cursor_position: (cursor.x, cursor.y),
        builtin_cursor: params.builtin_cursor && state.wants_default_cursor(),
        prompt: state.prompt.is_open().then(|| state.prompt.clone()),
        screenshot,
    })
}

/// After the frame is on its way to the screen: tell clients so that they
/// draw their next one, and do the per-frame housekeeping.
pub fn finish(state: &mut State, output: &Output) -> Result<(), Box<dyn Error>> {
    let elapsed = state.start_time.elapsed();
    for entry in &state.windows {
        entry
            .window
            .send_frame(output, elapsed, Some(Duration::ZERO), |_, _| {
                Some(output.clone())
            });
    }

    if let Some(surface) = state.cursor_surface() {
        send_frames_surface_tree(surface, output, elapsed, None, |_, _| Some(output.clone()));
    }

    state.refresh();
    state.display_handle.flush_clients()?;
    Ok(())
}
