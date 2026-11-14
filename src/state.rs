//! Compositor state: the Smithay protocol states, the seat, and the windows.

use std::{ffi::OsString, sync::Arc, time::Instant};

use smithay::{
    backend::{renderer::gles::GlesRenderer, winit::WinitGraphicsBackend},
    desktop::{PopupManager, Space, Window, WindowSurfaceType},
    input::{Seat, SeatState},
    reexports::{
        calloop::{EventLoop, Interest, LoopSignal, Mode, PostAction, generic::Generic},
        wayland_server::{
            Display, DisplayHandle,
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::wl_surface::WlSurface,
        },
    },
    utils::{Logical, Point, SERIAL_COUNTER},
    wayland::{
        compositor::{CompositorClientState, CompositorState},
        dmabuf::{DmabufGlobal, DmabufState},
        output::OutputManagerState,
        selection::data_device::DataDeviceState,
        shell::xdg::XdgShellState,
        shm::ShmState,
        socket::ListeningSocketSource,
    },
};

use crate::handlers;

/// Bookkeeping for `--smoke` runs. See `main.rs`.
pub struct Smoke {
    /// Whether a client window has to be on screen for a frame to count.
    pub needs_window: bool,
    pub frames: u32,
    pub passed: bool,
}

pub struct State {
    pub start_time: Instant,
    pub socket_name: OsString,
    pub display_handle: DisplayHandle,
    pub loop_signal: LoopSignal,

    /// The nested window we render into, and the GL renderer that owns its context.
    pub backend: WinitGraphicsBackend<GlesRenderer>,

    /// Flat 2D arrangement of windows. Replaced by the 3D world in M2.
    pub space: Space<Window>,
    pub popups: PopupManager,

    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub shm_state: ShmState,
    pub dmabuf_state: DmabufState,
    pub _dmabuf_global: DmabufGlobal,
    pub _output_manager_state: OutputManagerState,
    pub seat_state: SeatState<State>,
    pub data_device_state: DataDeviceState,

    pub seat: Seat<State>,

    pub smoke: Option<Smoke>,
}

impl State {
    pub fn new(
        event_loop: &mut EventLoop<State>,
        display: Display<State>,
        mut backend: WinitGraphicsBackend<GlesRenderer>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let dh = display.handle();

        let compositor_state = CompositorState::new::<Self>(&dh);
        let xdg_shell_state = XdgShellState::new::<Self>(&dh);
        let shm_state = ShmState::new::<Self>(&dh, vec![]);
        let (dmabuf_state, dmabuf_global) = handlers::dmabuf::init(&dh, backend.renderer());
        let output_manager_state = OutputManagerState::new_with_xdg_output::<Self>(&dh);
        let data_device_state = DataDeviceState::new::<Self>(&dh);

        let mut seat_state = SeatState::new();
        let mut seat: Seat<Self> = seat_state.new_wl_seat(&dh, "seat0");
        // 200 ms delay, 25 keys/s repeat. Devices are assumed present; real
        // hot-plug tracking arrives with the libinput backend.
        seat.add_keyboard(Default::default(), 200, 25)?;
        seat.add_pointer();

        let socket_name = Self::listen(display, event_loop)?;

        Ok(Self {
            start_time: Instant::now(),
            socket_name,
            display_handle: dh,
            loop_signal: event_loop.get_signal(),
            backend,
            space: Space::default(),
            popups: PopupManager::default(),
            compositor_state,
            xdg_shell_state,
            shm_state,
            dmabuf_state,
            _dmabuf_global: dmabuf_global,
            _output_manager_state: output_manager_state,
            seat_state,
            data_device_state,
            seat,
            smoke: None,
        })
    }

    /// Open the Wayland socket and hook client dispatch into the event loop.
    fn listen(
        display: Display<State>,
        event_loop: &mut EventLoop<State>,
    ) -> Result<OsString, Box<dyn std::error::Error>> {
        let socket = ListeningSocketSource::new_auto()?;
        let socket_name = socket.socket_name().to_os_string();
        let handle = event_loop.handle();

        handle.insert_source(socket, |stream, _, state| {
            if let Err(err) = state
                .display_handle
                .insert_client(stream, Arc::new(ClientState::default()))
            {
                tracing::warn!(%err, "failed to accept client");
            }
        })?;

        handle.insert_source(
            Generic::new(display, Interest::READ, Mode::Level),
            |_, display, state| {
                // Safety: the display is never dropped while the source is alive.
                unsafe { display.get_mut().dispatch_clients(state)? };
                Ok(PostAction::Continue)
            },
        )?;

        Ok(socket_name)
    }

    /// The window whose toplevel surface is `surface`, if any.
    pub fn window_for_toplevel(&self, surface: &WlSurface) -> Option<&Window> {
        self.space
            .elements()
            .find(|window| window.toplevel().is_some_and(|t| t.wl_surface() == surface))
    }

    /// The surface (toplevel, subsurface or popup) under a point in the space,
    /// together with that surface's origin in space coordinates.
    pub fn surface_under(
        &self,
        pos: Point<f64, Logical>,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        let (window, location) = self.space.element_under(pos)?;
        let (surface, offset) =
            window.surface_under(pos - location.to_f64(), WindowSurfaceType::ALL)?;
        Some((surface, (offset + location).to_f64()))
    }

    /// Give keyboard focus to `window` and raise it, or clear focus with `None`.
    pub fn focus_window(&mut self, window: Option<&Window>) {
        let serial = SERIAL_COUNTER.next_serial();
        let keyboard = self.seat.get_keyboard().expect("seat has a keyboard");

        match window {
            Some(window) => {
                // Raising with `activate` also deactivates every other window.
                self.space.raise_element(window, true);
                let surface = window.toplevel().map(|t| t.wl_surface().clone());
                keyboard.set_focus(self, surface, serial);
            }
            None => {
                for window in self.space.elements() {
                    window.set_activated(false);
                }
                keyboard.set_focus(self, None, serial);
            }
        }

        // Tell clients whose activated state just changed. Windows that have not
        // had their initial configure yet get this state with it instead.
        for window in self.space.elements() {
            if let Some(toplevel) = window.toplevel()
                && toplevel.is_initial_configure_sent()
            {
                toplevel.send_pending_configure();
            }
        }
    }
}

/// Frames a `--smoke` run has to render before it passes.
const SMOKE_FRAMES: u32 = 60;

impl State {
    /// Count a rendered frame towards the smoke test and stop once it passes.
    pub fn smoke_frame_rendered(&mut self) {
        let Some(smoke) = self.smoke.as_mut() else {
            return;
        };
        // A window only has a bounding box once its client attached a buffer.
        let window_shown = self.space.elements().any(|w| !w.bbox().is_empty());
        if smoke.needs_window && !window_shown {
            return;
        }

        smoke.frames += 1;
        if smoke.frames >= SMOKE_FRAMES && !smoke.passed {
            smoke.passed = true;
            tracing::info!(frames = smoke.frames, "smoke test passed");
            self.loop_signal.stop();
        }
    }
}

/// Per-client data, one instance for each connected Wayland client.
#[derive(Default)]
pub struct ClientState {
    pub compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}
