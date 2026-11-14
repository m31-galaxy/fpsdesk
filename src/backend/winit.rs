//! Nested backend: FPSDesk runs as a window on a host Wayland (or X11) session.
//!
//! The window is our one output, the host's input events are our input
//! devices, and the host's frame pacing drives our redraws.

use std::{error::Error, time::Duration};

use smithay::{
    backend::{
        renderer::{
            Color32F, damage::OutputDamageTracker, element::surface::WaylandSurfaceRenderElement,
            gles::GlesRenderer,
        },
        winit::{WinitEvent, WinitEventLoop},
    },
    desktop::space::render_output,
    output::{Mode, Output, PhysicalProperties, Scale, Subpixel},
    reexports::calloop::EventLoop,
    utils::{Rectangle, Transform},
};
use tracing::error;

use crate::state::State;

const CLEAR_COLOR: Color32F = Color32F::new(0.05, 0.07, 0.12, 1.0);

/// The host does not tell us its refresh rate; advertise 60 Hz to clients.
const REFRESH_MHZ: i32 = 60_000;

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
    // GL's framebuffer origin is bottom-left, hence the flip. Matching the
    // host's scale keeps clients sharp on a HiDPI display.
    let scale = Scale::Fractional(state.backend.window().scale_factor());
    output.change_current_state(
        Some(mode),
        Some(Transform::Flipped180),
        Some(scale),
        Some((0, 0).into()),
    );
    output.set_preferred(mode);
    state.space.map_output(&output, (0, 0));

    let mut damage_tracker = OutputDamageTracker::from_output(&output);

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
                if let Err(err) = redraw(state, &output, &mut damage_tracker) {
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
    damage_tracker: &mut OutputDamageTracker,
) -> Result<(), Box<dyn Error>> {
    let damage = Rectangle::from_size(state.backend.window_size());

    {
        let (renderer, mut framebuffer) = state.backend.bind()?;
        // Buffer age 0: redraw everything each frame. Damage tracking is not
        // worth having until the 3D pipeline settles what a frame is.
        render_output::<_, WaylandSurfaceRenderElement<GlesRenderer>, _, _>(
            output,
            renderer,
            &mut framebuffer,
            1.0,
            0,
            [&state.space],
            &[],
            damage_tracker,
            CLEAR_COLOR,
        )?;
    }
    state.backend.submit(Some(&[damage]))?;

    // Tell clients the frame is on screen so they draw their next one.
    let elapsed = state.start_time.elapsed();
    for window in state.space.elements() {
        window.send_frame(output, elapsed, Some(Duration::ZERO), |_, _| {
            Some(output.clone())
        });
    }

    state.space.refresh();
    state.popups.cleanup();
    state.display_handle.flush_clients()?;

    state.smoke_frame_rendered();

    // Redraw continuously; the host throttles us to its refresh rate.
    state.backend.window().request_redraw();
    Ok(())
}
