//! Compositor state: the Smithay protocol states, the seat, and the windows.

use std::{collections::HashSet, ffi::OsString, path::PathBuf, sync::Arc, time::Instant};

use glam::{Vec2, Vec3};
use smithay::{
    desktop::{PopupManager, Window, space::SpaceElement},
    input::{Seat, SeatState, pointer::CursorImageStatus},
    output::Output,
    reexports::{
        calloop::{self, EventLoop, Interest, LoopSignal, PostAction, generic::Generic},
        wayland_server::{
            Display, DisplayHandle,
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::wl_surface::WlSurface,
        },
    },
    utils::{IsAlive, Logical, Point, SERIAL_COUNTER},
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

use crate::{
    backend::Backend,
    handlers,
    input::controls::{Mode, MoveKeys},
    render::{
        scene::SceneRenderer,
        window::{Extent, WindowTexture},
    },
    script::Script,
    world::{self, Camera, EYE_HEIGHT, Panel},
};

/// A client window and its place in the world.
pub struct WindowEntry {
    pub window: Window,
    /// Which `world::arc_slot` the window occupies.
    pub slot: usize,
    pub panel: Panel,
    /// The area the window covered when last drawn; `None` until then.
    pub extent: Option<Extent>,
    /// The window's flattened picture; `None` until it first has content.
    pub texture: Option<WindowTexture>,
    /// Whether to draw attention to the window this frame.
    pub highlight: Highlight,
}

impl WindowEntry {
    /// The panel the window's picture is on: its own panel, grown to take in
    /// any open popups. `None` until the window has been drawn.
    pub fn content_panel(&self) -> Option<Panel> {
        let Extent { geometry, content } = self.extent?;
        let size = Vec2::new(geometry.size.w as f32, geometry.size.h as f32);
        let offset = content.loc - geometry.loc;
        let min = Vec2::new(offset.x as f32, offset.y as f32) / size;
        let max = min + Vec2::new(content.size.w as f32, content.size.h as f32) / size;
        Some(self.panel.region(min, max))
    }

    /// The point in window coordinates that `uv`, a position on the content
    /// panel as in `world::Hit`, corresponds to.
    pub fn window_point(&self, uv: Vec2) -> Option<Point<f64, Logical>> {
        let content = self.extent?.content.to_f64();
        Some(Point::from((
            content.loc.x + f64::from(uv.x) * content.size.w,
            content.loc.y + f64::from(uv.y) * content.size.h,
        )))
    }
}

/// Why a window is outlined.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Highlight {
    None,
    /// The crosshair is on it in walk mode.
    Hovered,
    /// It has keyboard focus in interact mode.
    Focused,
}

/// Bookkeeping for `--smoke` runs. See `main.rs`.
pub struct Smoke {
    /// Whether a client window has to be on screen for a frame to count.
    pub needs_window: bool,
    pub frames: u32,
    pub passed: bool,
    /// Where to save the final frame, if anywhere.
    pub screenshot: Option<PathBuf>,
}

pub struct State {
    pub start_time: Instant,
    pub socket_name: OsString,
    pub display_handle: DisplayHandle,
    pub loop_signal: LoopSignal,

    /// The backend that owns the output and the GL renderer.
    pub backend: Backend,
    pub scene: SceneRenderer,
    /// Outputs the backend has created.
    pub outputs: Vec<Output>,

    pub mode: Mode,
    pub move_keys: MoveKeys,
    /// Pointer movement, in logical pixels, not yet applied to the camera.
    pub look_motion: Vec2,
    /// Whether the backend delivers relative pointer motion. Without it,
    /// mouselook falls back to differences between absolute positions.
    pub has_relative_pointer: bool,
    pub last_pointer_position: Option<Vec2>,
    /// Keys whose press we kept for ourselves, by xkb key code.
    pub suppressed_keys: HashSet<u32>,

    /// Where the pointer is on the output in interact mode, in logical pixels.
    pub cursor: Vec2,
    /// How the client under the pointer wants the cursor to look.
    pub cursor_status: CursorImageStatus,
    /// Pointer buttons held down, by evdev code.
    pub pressed_buttons: HashSet<u32>,
    /// Buttons whose press we kept for ourselves.
    pub suppressed_buttons: HashSet<u32>,
    /// The window a held button went down on. Until the button is released
    /// the pointer stays in that window's coordinates, even past its edges,
    /// so that a drag carries on smoothly.
    pub drag_window: Option<Window>,

    pub camera: Camera,
    /// The window under the crosshair.
    pub hovered: Option<Window>,
    /// The window with keyboard focus.
    pub focused: Option<Window>,
    pub windows: Vec<WindowEntry>,
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
    /// Scripted input for automated checks. See `script.rs`.
    pub script: Option<Script>,
}

impl State {
    pub fn new(
        event_loop: &mut EventLoop<State>,
        display: Display<State>,
        mut backend: Backend,
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

        let scene = SceneRenderer::new(backend.renderer())?;

        // Windows are arranged around the origin, so start in the middle of
        // them, looking at the first.
        let camera = Camera::new(Vec3::new(0.0, EYE_HEIGHT, 0.0), 0.0, 0.0);

        let socket_name = Self::listen(display, event_loop)?;

        Ok(Self {
            start_time: Instant::now(),
            socket_name,
            display_handle: dh,
            loop_signal: event_loop.get_signal(),
            backend,
            scene,
            outputs: Vec::new(),
            mode: Mode::Walk,
            move_keys: MoveKeys::default(),
            look_motion: Vec2::ZERO,
            has_relative_pointer: false,
            last_pointer_position: None,
            suppressed_keys: HashSet::new(),
            cursor: Vec2::ZERO,
            cursor_status: CursorImageStatus::default_named(),
            pressed_buttons: HashSet::new(),
            suppressed_buttons: HashSet::new(),
            drag_window: None,
            camera,
            hovered: None,
            focused: None,
            windows: Vec::new(),
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
            script: None,
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
            Generic::new(display, Interest::READ, calloop::Mode::Level),
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
        self.windows
            .iter()
            .map(|entry| &entry.window)
            .find(|window| window.toplevel().is_some_and(|t| t.wl_surface() == surface))
    }

    /// Start managing a new window: give it a place in the world.
    pub fn add_window(&mut self, window: Window) {
        let slot = (0..)
            .find(|slot| self.windows.iter().all(|entry| entry.slot != *slot))
            .expect("there is always a free slot");
        self.windows.push(WindowEntry {
            window: window.clone(),
            slot,
            panel: world::arc_slot(slot),
            extent: None,
            texture: None,
            highlight: Highlight::None,
        });
        // While the user is working in an application, a window it opens (a
        // dialog, say) should take over the keyboard. While they are walking,
        // nothing should.
        if self.mode == Mode::Interact {
            self.focus_window(Some(&window));
        }
    }

    /// Give keyboard focus to `window`, or clear focus with `None`.
    pub fn focus_window(&mut self, window: Option<&Window>) {
        let serial = SERIAL_COUNTER.next_serial();
        let keyboard = self.seat.get_keyboard().expect("seat has a keyboard");

        self.focused = window.cloned();
        for entry in &self.windows {
            entry.window.set_activated(Some(&entry.window) == window);
        }
        let surface = window
            .and_then(|window| window.toplevel())
            .map(|toplevel| toplevel.wl_surface().clone());
        keyboard.set_focus(self, surface, serial);

        // Tell clients whose activated state just changed. Windows that have not
        // had their initial configure yet get this state with it instead.
        for entry in &self.windows {
            if let Some(toplevel) = entry.window.toplevel()
                && toplevel.is_initial_configure_sent()
            {
                toplevel.send_pending_configure();
            }
        }
    }

    /// Per-frame housekeeping: forget closed windows and keep the rest
    /// informed about the output they are shown on.
    pub fn refresh(&mut self) {
        self.windows.retain(|entry| entry.window.alive());

        if self.hovered.as_ref().is_some_and(|window| !window.alive()) {
            self.hovered = None;
        }
        // Closing the window you were using puts you back on your feet.
        if self.focused.as_ref().is_some_and(|window| !window.alive()) {
            self.enter_walk();
        }

        for entry in &self.windows {
            // Every window counts as fully on every output, wherever its panel
            // is: this is what tells clients which scale to draw at.
            for output in &self.outputs {
                entry.window.output_enter(output, entry.window.bbox());
            }
            entry.window.refresh();
        }
        self.popups.cleanup();
    }
}

/// Frames a `--smoke` run has to render before it passes.
const SMOKE_FRAMES: u32 = 60;

impl State {
    /// Count a rendered frame towards the smoke test. Returns true for the
    /// frame that completes it, and stops the event loop.
    pub fn smoke_frame_rendered(&mut self) -> bool {
        let Some(smoke) = self.smoke.as_mut() else {
            return false;
        };
        // Frames only count once any scripted input has finished playing, so
        // that a screenshot shows its result.
        if self
            .script
            .as_ref()
            .is_some_and(|script| !script.is_finished())
        {
            return false;
        }
        // A window gets its texture the first time it is drawn into the world.
        let window_shown = self.windows.iter().any(|entry| entry.texture.is_some());
        if smoke.passed || (smoke.needs_window && !window_shown) {
            return false;
        }

        smoke.frames += 1;
        if smoke.frames < SMOKE_FRAMES {
            return false;
        }
        smoke.passed = true;
        tracing::info!(frames = smoke.frames, "smoke test passed");
        self.loop_signal.stop();
        true
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
