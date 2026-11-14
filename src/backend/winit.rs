//! Nested backend: FPSDesk runs as a window on a host Wayland (or X11) session.
//!
//! The window is our one output, the host's input events are our input
//! devices, and the host's frame pacing drives our redraws.

use std::{error::Error, time::Duration};

use smithay::{
    backend::winit::{WinitEvent, WinitEventLoop},
    output::{Mode, Output, PhysicalProperties, Scale, Subpixel},
    reexports::calloop::EventLoop,
    utils::{Rectangle, Transform},
};
use tracing::{error, info};

use crate::{
    render::{self, Presenter, screenshot},
    state::State,
};

/// The host does not tell us its refresh rate; advertise 60 Hz to clients.
const REFRESH_MHZ: i32 = 60_000;

/// GL puts a window surface's origin at the bottom left, so what we draw is
/// upside down relative to how the output is described to clients.
const OUTPUT_TRANSFORM: Transform = Transform::Flipped180;

/// Create the output for the nested window and start handling its events.
pub fn init(
    event_loop: &mut EventLoop<State>,
    state: &mut State,
    winit: WinitEventLoop,
) -> Result<(), Box<dyn Error>> {
    state.backend.window().set_title("FPSDesk");

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

    let mut presenter = Presenter::new();

    event_loop
        .handle()
        .insert_source(winit, move |event, _, state| match event {
            WinitEvent::Resized { size, scale_factor } => {
                let mode = Mode {
                    size,
                    refresh: REFRESH_MHZ,
                };
                output.change_current_state(
                    Some(mode),
                    None,
                    Some(Scale::Fractional(scale_factor)),
                    None,
                );
            }
            WinitEvent::Input(event) => state.process_input_event(event),
            WinitEvent::Redraw => {
                if let Err(err) = redraw(state, &output, &mut presenter) {
                    error!(%err, "rendering failed");
                    state.loop_signal.stop();
                }
            }
            WinitEvent::CloseRequested => state.loop_signal.stop(),
            WinitEvent::Focus(_) => {}
        })?;

    Ok(())
}

fn redraw(
    state: &mut State,
    output: &Output,
    presenter: &mut Presenter,
) -> Result<(), Box<dyn Error>> {
    let size = state.backend.window_size();
    let scale = output.current_scale().fractional_scale();

    let world = render::render_world(
        state.backend.renderer(),
        &mut state.scene,
        &mut state.windows,
        &state.camera,
        size,
        scale,
    )?;

    let smoke_finished = state.smoke_frame_rendered();
    {
        let (renderer, mut framebuffer) = state.backend.bind()?;
        presenter.present(renderer, &mut framebuffer, size, OUTPUT_TRANSFORM, world)?;

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
            .send_frame(output, elapsed, Some(Duration::ZERO), |_, _| {
                Some(output.clone())
            });
    }

    state.refresh();
    state.display_handle.flush_clients()?;

    // Redraw continuously; the host throttles us to its refresh rate.
    state.backend.window().request_redraw();
    Ok(())
}
