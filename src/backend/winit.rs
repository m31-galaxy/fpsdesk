//! Nested backend: FPSDesk runs as a window on a host Wayland (or X11) session.
//!
//! The window is our one output, the host's input events are our input
//! devices, and the host's frame pacing drives our redraws.

use std::{
    error::Error,
    mem::ManuallyDrop,
    time::{Duration, Instant},
};

use glam::Vec2;
use smithay::{
    backend::winit::{WinitEvent, WinitEventLoop},
    desktop::utils::send_frames_surface_tree,
    output::{Mode, Output, PhysicalProperties, Scale, Subpixel},
    reexports::{
        calloop::EventLoop,
        winit::{dpi::PhysicalPosition, window::CursorGrabMode},
    },
    utils::{Rectangle, Transform},
};
use tracing::{error, info, warn};

use super::relative_pointer::RelativePointer;
use crate::{
    frame_stats::FrameStats,
    input::controls::Mode as ControlMode,
    render::{self, CursorSprite, Overlays, Presenter, screenshot},
    state::State,
};

/// The host does not tell us its refresh rate; advertise 60 Hz to clients.
const REFRESH_MHZ: i32 = 60_000;

/// GL puts a window surface's origin at the bottom left, so what we draw is
/// upside down relative to how the output is described to clients.
const OUTPUT_TRANSFORM: Transform = Transform::Flipped180;

/// The longest step the world takes in one frame. After a stall (the window
/// was hidden, say) this stops a held key from teleporting the viewer.
const MAX_FRAME_STEP: Duration = Duration::from_millis(100);

/// What the nested backend keeps between frames.
struct Nested {
    output: Output,
    presenter: Presenter,
    stats: FrameStats,
    last_frame: Instant,
    /// Relative motion from the host, if it offers it. Never dropped: see
    /// `RelativePointer::new`.
    relative_pointer: Option<ManuallyDrop<RelativePointer>>,
    /// Whether the host's own cursor is showing over our window.
    host_cursor_visible: bool,
    /// Whether we currently hold the host's pointer.
    pointer_captured: bool,
}

/// Create the output for the nested window and start handling its events.
pub fn init(
    event_loop: &mut EventLoop<State>,
    state: &mut State,
    winit: WinitEventLoop,
) -> Result<(), Box<dyn Error>> {
    state
        .backend
        .window()
        .set_title("FPSDesk (Ctrl+Esc releases the mouse)");

    let output = Output::new(
        "winit".to_string(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "FPSDesk".into(),
            model: "Nested window".into(),
            serial_number: "Unknown".into(),
        },
    );
    let _global = output.create_global::<State>(&state.display_handle);

    let mode = Mode {
        size: state.backend.window_size(),
        refresh: REFRESH_MHZ,
    };
    // Matching the host's scale keeps clients sharp on a HiDPI display.
    let scale = Scale::Fractional(state.backend.window().scale_factor());
    output.change_current_state(
        Some(mode),
        Some(OUTPUT_TRANSFORM),
        Some(scale),
        Some((0, 0).into()),
    );
    output.set_preferred(mode);
    state.outputs.push(output.clone());

    // Safety: wrapped in `ManuallyDrop` below and so never dropped, while the
    // winit event loop lives in the calloop loop until the process exits.
    let relative_pointer = match unsafe { RelativePointer::new(state.backend.window()) } {
        Ok(relative_pointer) => {
            info!("mouselook uses the host's relative pointer motion");
            Some(ManuallyDrop::new(relative_pointer))
        }
        Err(err) => {
            warn!(%err, "no relative pointer motion from the host; mouselook will stop at the window's edges");
            None
        }
    };
    state.has_relative_pointer = relative_pointer.is_some();

    let mut nested = Nested {
        output,
        presenter: Presenter::new(),
        stats: FrameStats::new(),
        last_frame: Instant::now(),
        relative_pointer,
        pointer_captured: false,
        host_cursor_visible: true,
    };

    event_loop
        .handle()
        .insert_source(winit, move |event, _, state| match event {
            WinitEvent::Resized { size, scale_factor } => {
                let mode = Mode {
                    size,
                    refresh: REFRESH_MHZ,
                };
                nested.output.change_current_state(
                    Some(mode),
                    None,
                    Some(Scale::Fractional(scale_factor)),
                    None,
                );
            }
            WinitEvent::Input(event) => state.process_input_event(event),
            WinitEvent::Redraw => {
                if let Err(err) = redraw(state, &mut nested) {
                    error!(%err, "rendering failed");
                    state.loop_signal.stop();
                }
            }
            WinitEvent::CloseRequested => state.loop_signal.stop(),
            WinitEvent::Focus(focused) => {
                if !focused {
                    // The host keeps key releases from an unfocused window.
                    state.release_movement();
                }
            }
        })?;

    Ok(())
}

fn redraw(state: &mut State, nested: &mut Nested) -> Result<(), Box<dyn Error>> {
    let frame_start = Instant::now();
    let dt = frame_start
        .duration_since(nested.last_frame)
        .min(MAX_FRAME_STEP);
    nested.last_frame = frame_start;

    let size = state.backend.window_size();
    let scale = nested.output.current_scale().fractional_scale();

    state.run_script_step();
    sync_host_pointer(state, nested);
    if let Some(relative_pointer) = nested.relative_pointer.as_mut() {
        // Taken every frame, used or not, so that motion made in interact
        // mode does not pile up and swing the view on return to walking.
        let (dx, dy) = relative_pointer.take_motion();
        state.look(Vec2::new(dx as f32, dy as f32));
    }
    state.advance(dt.as_secs_f32());

    let world = render::render_world(
        state.backend.renderer(),
        &mut state.scene,
        &mut state.windows,
        &state.shooting.corpses,
        &state.camera,
        size,
        scale,
    )?;

    let smoke_finished = state.smoke_frame_rendered();
    {
        // Cloned so that it does not hold a borrow of `state` across `bind`.
        let cursor_surface = state.cursor_surface().cloned();
        let overlays = Overlays {
            weapon: state.weapon_view(),
            crosshair: state.mode == ControlMode::Walk,
            cursor: cursor_surface.as_ref().map(|surface| CursorSprite {
                surface,
                position: <(i32, i32)>::from((state.cursor.as_dvec2() * scale).round().as_ivec2())
                    .into(),
                scale,
            }),
        };
        let (renderer, mut framebuffer) = state.backend.bind()?;
        nested.presenter.present(
            renderer,
            &mut framebuffer,
            size,
            OUTPUT_TRANSFORM,
            world,
            overlays,
        )?;

        let screenshot = state.smoke.as_ref().and_then(|s| s.screenshot.as_deref());
        if smoke_finished && let Some(path) = screenshot {
            screenshot::save(renderer, &mut framebuffer, size, path)?;
            info!(?path, "saved screenshot");
        }
    }
    state.backend.submit(Some(&[Rectangle::from_size(size)]))?;

    // Tell clients the frame is on screen so they draw their next one.
    let elapsed = state.start_time.elapsed();
    for entry in &state.windows {
        entry
            .window
            .send_frame(&nested.output, elapsed, Some(Duration::ZERO), |_, _| {
                Some(nested.output.clone())
            });
    }

    if let Some(surface) = state.cursor_surface() {
        send_frames_surface_tree(surface, &nested.output, elapsed, None, |_, _| {
            Some(nested.output.clone())
        });
    }

    state.refresh();
    state.display_handle.flush_clients()?;
    nested.stats.frame(frame_start.elapsed());

    // Redraw continuously; the host throttles us to its refresh rate.
    state.backend.window().request_redraw();
    Ok(())
}

/// Make the host's pointer match our mode: locked and hidden while walking,
/// free in interact mode, and visible only when no client draws its own.
fn sync_host_pointer(state: &mut State, nested: &mut Nested) {
    let capture = state.wants_pointer_capture();
    let visible = state.wants_default_cursor();
    let size = state.backend.window_size();
    let window = state.backend.window();

    if capture != nested.pointer_captured {
        nested.pointer_captured = capture;
        if !capture {
            // Ask for the pointer to reappear in the middle of the window,
            // where the crosshair was, rather than wherever it was locked.
            // Only a hint, and only honoured while still locked.
            let centre = PhysicalPosition::new(size.w / 2, size.h / 2);
            let _ = window.set_cursor_position(centre.into());
        }
        let grab = if capture {
            CursorGrabMode::Locked
        } else {
            CursorGrabMode::None
        };
        if let Err(err) = window.set_cursor_grab(grab) {
            // Not retried: `pointer_captured` already records the attempt.
            warn!(%err, "the host would not let us lock the pointer");
        }
    }

    if visible != nested.host_cursor_visible {
        nested.host_cursor_visible = visible;
        window.set_cursor_visible(visible);
    }
}
