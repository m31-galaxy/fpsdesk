//! Input routing and the control modes.
//!
//! In **walk** mode the keyboard and mouse move the viewer and no client has
//! keyboard or pointer focus. In **interact** mode keys go to the focused
//! window and the mouse drives a cursor on the screen; whatever window
//! surface is behind the cursor, traced through the camera, gets the pointer.
//!
//! Bindings:
//! - Walk: W/A/S/D or arrows move, Shift runs, the mouse looks. Left click or
//!   Enter on the window under the crosshair focuses it and enters interact.
//! - Interact: clicking a window focuses it.
//! - Either mode: Super, or Ctrl+Escape, switches to the other mode. (Under a
//!   host desktop Super usually never reaches us, hence the second binding.)
//!
//! The handlers take plain values rather than backend events so that scripted
//! input (`script.rs`) can drive them exactly as a real device would.

pub mod controls;

use glam::Vec2;
use smithay::{
    backend::input::{
        AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, InputBackend, InputEvent,
        InputTime, KeyState, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent,
        PointerMotionEvent,
    },
    desktop::{Window, WindowSurfaceType},
    input::{
        keyboard::{FilterResult, Keycode, Keysym},
        pointer::{AxisFrame, ButtonEvent, CursorImageStatus, MotionEvent},
    },
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{IsAlive, Logical, Point, SERIAL_COUNTER},
};
use tracing::info;

use crate::{
    state::{Highlight, State, WindowEntry},
    world::{self, Ray},
};
use controls::{Mode, key, look_delta};

/// The left mouse button's evdev code.
pub const BTN_LEFT: u32 = 0x110;

/// Scroll distance, in logical pixels, for one notch of a mouse wheel.
const WHEEL_NOTCH_PIXELS: f64 = 15.0;

/// What a key press that we kept for ourselves asks for.
enum KeyAction {
    ToggleMode,
    /// A key that means something in walk mode, or nothing at all.
    Walk,
}

/// A point on a window, found by following a ray into the world.
struct Target {
    window: Window,
    /// In window coordinates: logical pixels from the origin of the window's
    /// surface tree.
    location: Point<f64, Logical>,
}

impl Target {
    /// The surface at the target (the window, a subsurface or a popup) and
    /// that surface's origin in window coordinates.
    fn surface(&self) -> Option<(WlSurface, Point<f64, Logical>)> {
        self.window
            .surface_under(self.location, WindowSurfaceType::ALL)
            .map(|(surface, origin)| (surface, origin.to_f64()))
    }
}

impl State {
    pub fn process_input_event<I: InputBackend>(&mut self, event: InputEvent<I>) {
        match event {
            InputEvent::Keyboard { event, .. } => {
                self.key(event.key_code(), event.state(), event.time());
            }
            InputEvent::PointerMotion { event, .. } => {
                self.look(Vec2::new(event.delta_x() as f32, event.delta_y() as f32));
            }
            InputEvent::PointerMotionAbsolute { event, .. } => {
                if let Some(size) = self.view_size() {
                    let position =
                        event.position_transformed((size.x as i32, size.y as i32).into());
                    self.pointer_moved_to(
                        Vec2::new(position.x as f32, position.y as f32),
                        event.time(),
                    );
                }
            }
            InputEvent::PointerButton { event, .. } => {
                self.pointer_button(event.button_code(), event.state(), event.time());
            }
            InputEvent::PointerAxis { event, .. } => {
                let source = event.source();
                let mut frame = AxisFrame::new(event.time()).source(source);
                for axis in [Axis::Horizontal, Axis::Vertical] {
                    // Wheels report notches in 120ths; touchpads report pixels.
                    let v120 = event.amount_v120(axis);
                    let amount = event
                        .amount(axis)
                        .unwrap_or_else(|| v120.unwrap_or(0.0) * WHEEL_NOTCH_PIXELS / 120.0);
                    if amount != 0.0 {
                        frame = frame.value(axis, amount);
                        if let Some(v120) = v120 {
                            frame = frame.v120(axis, v120 as i32);
                        }
                    } else if source == AxisSource::Finger && event.amount(axis) == Some(0.0) {
                        // Fingers lifted: lets clients start kinetic scrolling.
                        frame = frame.stop(axis);
                    }
                }
                self.pointer_axis(frame);
            }
            _ => {}
        }
    }

    /// The size of the view in logical pixels.
    pub fn view_size(&self) -> Option<Vec2> {
        let output = self.outputs.first()?;
        let size = output.current_mode()?.size.to_f64();
        let scale = output.current_scale().fractional_scale();
        Some(Vec2::new((size.w / scale) as f32, (size.h / scale) as f32))
    }

    /// A key went down or up.
    pub fn key(&mut self, keycode: Keycode, key_state: KeyState, time: InputTime) {
        // xkb key codes are evdev codes offset by 8.
        let evdev_code = keycode.raw().saturating_sub(8);
        let pressed = key_state == KeyState::Pressed;

        let keyboard = self.seat.get_keyboard().expect("seat has a keyboard");
        let action = keyboard.input(
            self,
            keycode,
            key_state,
            SERIAL_COUNTER.next_serial(),
            time,
            |state, modifiers, handle| {
                if !pressed {
                    // The release of a key whose press we kept must not reach
                    // a client that never saw the press.
                    return if state.suppressed_keys.remove(&keycode.raw()) {
                        FilterResult::Intercept(KeyAction::Walk)
                    } else {
                        FilterResult::Forward
                    };
                }

                let sym = handle.modified_sym();
                let toggle = sym == Keysym::Super_L
                    || sym == Keysym::Super_R
                    || (sym == Keysym::Escape && modifiers.ctrl);
                let action = if toggle {
                    KeyAction::ToggleMode
                } else if state.mode == Mode::Walk {
                    KeyAction::Walk
                } else {
                    return FilterResult::Forward;
                };
                state.suppressed_keys.insert(keycode.raw());
                FilterResult::Intercept(action)
            },
        );

        match action {
            Some(KeyAction::ToggleMode) => self.toggle_mode(),
            Some(KeyAction::Walk) => {
                // Releases always count, so a key held while leaving walk mode
                // does not stay held.
                if self.mode == Mode::Walk || !pressed {
                    self.move_keys.set(evdev_code, pressed);
                }
                if pressed && evdev_code == key::ENTER && self.mode == Mode::Walk {
                    self.use_hovered();
                }
            }
            None => {}
        }
    }

    /// The pointer is now at `position` on the output, in logical pixels.
    pub fn pointer_moved_to(&mut self, position: Vec2, time: InputTime) {
        let last = self.last_pointer_position.replace(position);
        match self.mode {
            Mode::Walk => {
                // Absolute positions are all a nested window's host gives us
                // by default. If the backend has no better source of relative
                // motion, turn the differences into mouselook; the view then
                // stops turning when the pointer reaches the window's edge.
                if let Some(last) = last
                    && !self.has_relative_pointer
                {
                    self.look(position - last);
                }
            }
            Mode::Interact => {
                self.cursor = position;
                self.route_pointer(time);
            }
        }
    }

    /// A pointer button, given by evdev code, went down or up.
    pub fn pointer_button(&mut self, button: u32, button_state: ButtonState, time: InputTime) {
        let pointer = self.seat.get_pointer().expect("seat has a pointer");

        match button_state {
            ButtonState::Pressed => {
                self.pressed_buttons.insert(button);
                if self.mode == Mode::Walk {
                    // This click is ours; its release must not reach a client.
                    self.suppressed_buttons.insert(button);
                    if button == BTN_LEFT {
                        self.use_hovered();
                    }
                    return;
                }

                self.route_pointer(time);
                // The first button to go down picks the window: it takes the
                // keyboard, and holds the pointer until every button is up.
                if self.pressed_buttons.len() == 1
                    && let Some(target) = self.cursor_target()
                {
                    if self.focused.as_ref() != Some(&target.window) {
                        self.focus_window(Some(&target.window));
                    }
                    self.drag_window = Some(target.window);
                }
            }
            ButtonState::Released => {
                self.pressed_buttons.remove(&button);
                if self.suppressed_buttons.remove(&button) {
                    return;
                }
            }
        }

        pointer.button(
            self,
            &ButtonEvent {
                serial: SERIAL_COUNTER.next_serial(),
                time,
                button,
                state: button_state,
            },
        );
        pointer.frame(self);

        if self.pressed_buttons.is_empty() && self.drag_window.take().is_some() {
            // The pointer may have ended up over something else.
            self.route_pointer(time);
        }
    }

    /// The wheel turned or fingers scrolled.
    pub fn pointer_axis(&mut self, frame: AxisFrame) {
        if self.mode == Mode::Interact {
            let pointer = self.seat.get_pointer().expect("seat has a pointer");
            pointer.axis(self, frame);
            pointer.frame(self);
        }
    }

    /// Turn the view by a pointer movement of `motion` logical pixels.
    /// Ignored outside walk mode.
    pub fn look(&mut self, motion: Vec2) {
        if self.mode == Mode::Walk {
            self.look_motion += motion;
        }
    }

    /// Forget held movement keys, for when key releases may have been missed.
    pub fn release_movement(&mut self) {
        self.move_keys.release_all();
    }

    /// Whether the backend should confine and hide the pointer.
    pub fn wants_pointer_capture(&self) -> bool {
        self.mode == Mode::Walk
    }

    /// The cursor image a client has supplied, if that is what should be
    /// drawn at the pointer right now.
    pub fn cursor_surface(&self) -> Option<&WlSurface> {
        match &self.cursor_status {
            CursorImageStatus::Surface(surface)
                if self.mode == Mode::Interact && surface.alive() =>
            {
                Some(surface)
            }
            _ => None,
        }
    }

    /// Whether the backend should show its own standard cursor: in interact
    /// mode, unless a client supplies an image or asks for none.
    pub fn wants_default_cursor(&self) -> bool {
        self.mode == Mode::Interact
            && match &self.cursor_status {
                CursorImageStatus::Named(_) => true,
                CursorImageStatus::Hidden => false,
                // A cursor surface that has gone away cannot be drawn.
                CursorImageStatus::Surface(surface) => !surface.alive(),
            }
    }

    pub fn toggle_mode(&mut self) {
        match self.mode {
            Mode::Walk => self.enter_interact(self.hovered.clone()),
            Mode::Interact => self.enter_walk(),
        }
    }

    /// Focus the window under the crosshair, if there is one.
    pub fn use_hovered(&mut self) {
        if let Some(window) = self.hovered.clone() {
            self.enter_interact(Some(window));
        }
    }

    pub fn enter_walk(&mut self) {
        info!("walk mode");
        self.mode = Mode::Walk;
        self.look_motion = Vec2::ZERO;
        self.drag_window = None;
        self.focus_window(None);

        // Take the pointer away from whichever client had it.
        let pointer = self.seat.get_pointer().expect("seat has a pointer");
        pointer.motion(
            self,
            None,
            &MotionEvent {
                location: pointer.current_location(),
                serial: SERIAL_COUNTER.next_serial(),
                time: InputTime::now(),
            },
        );
        pointer.frame(self);
    }

    pub fn enter_interact(&mut self, window: Option<Window>) {
        info!(focused = window.is_some(), "interact mode");
        self.mode = Mode::Interact;
        self.move_keys.release_all();
        self.look_motion = Vec2::ZERO;
        self.focus_window(window.as_ref());

        // The cursor starts where the crosshair was: the middle of the view.
        if let Some(size) = self.view_size() {
            self.cursor = size / 2.0;
        }
        self.route_pointer(InputTime::now());
    }

    /// Advance the world by `dt` seconds: apply movement and mouselook, then
    /// work out what the crosshair or cursor is on.
    pub fn advance(&mut self, dt: f32) {
        if self.mode == Mode::Walk {
            let (yaw, pitch) = look_delta(std::mem::take(&mut self.look_motion));
            self.camera.turn(yaw, pitch);
            self.camera.walk(self.move_keys.step(dt));
        }

        self.hovered = match self.mode {
            // The crosshair is the centre of the screen.
            Mode::Walk => self
                .ray_through(Vec2::ZERO)
                .and_then(|ray| self.pick(&ray))
                .map(|target| target.window),
            Mode::Interact => None,
        };

        if self.mode == Mode::Interact {
            self.refresh_pointer_focus();
        }

        for entry in &mut self.windows {
            let is = |window: &Option<Window>| window.as_ref() == Some(&entry.window);
            entry.highlight = match self.mode {
                Mode::Walk if is(&self.hovered) => Highlight::Hovered,
                Mode::Interact if is(&self.focused) => Highlight::Focused,
                _ => Highlight::None,
            };
        }
    }

    /// The ray from the eye through a point on the screen given in normalised
    /// device coordinates (see `Camera::ray_through`).
    fn ray_through(&self, ndc: Vec2) -> Option<Ray> {
        let size = self.view_size()?;
        Some(self.camera.ray_through(ndc, size.x / size.y))
    }

    /// The ray from the eye through the cursor.
    fn cursor_ray(&self) -> Option<Ray> {
        let size = self.view_size()?;
        // Screen y runs downwards, device y upwards.
        let ndc = Vec2::new(
            2.0 * self.cursor.x / size.x - 1.0,
            1.0 - 2.0 * self.cursor.y / size.y,
        );
        self.ray_through(ndc)
    }

    /// The nearest window surface that `ray` lands on.
    fn pick(&self, ray: &Ray) -> Option<Target> {
        let shown: Vec<(&WindowEntry, world::Panel)> = self
            .windows
            .iter()
            .filter_map(|entry| Some((entry, entry.content_panel()?)))
            .collect();
        let target = |index: usize, hit: &world::Hit| {
            let entry: &WindowEntry = shown[index].0;
            Some(Target {
                window: entry.window.clone(),
                location: entry.window_point(hit.uv)?,
            })
        };

        // A window's picture is a rectangle around it and its popups, with
        // gaps where there is neither. Only actual surfaces stop the ray.
        let (index, hit) = world::pick(ray, shown.iter().map(|(_, panel)| panel), |index, hit| {
            target(index, hit).is_some_and(|target| target.surface().is_some())
        })?;
        target(index, &hit)
    }

    /// What the cursor is on. While a button is held this stays with the
    /// window the button went down on, following that window's plane past
    /// its edges, so that a drag (a selection, a scrollbar) keeps getting
    /// sensible coordinates when the cursor leaves the window.
    fn cursor_target(&self) -> Option<Target> {
        let ray = self.cursor_ray()?;
        let Some(window) = &self.drag_window else {
            return self.pick(&ray);
        };
        let entry = self.windows.iter().find(|entry| &entry.window == window)?;
        let hit = entry.content_panel()?.intersect_plane(&ray)?;
        Some(Target {
            window: window.clone(),
            location: entry.window_point(hit.uv)?,
        })
    }

    /// Tell the seat where the pointer is, which moves pointer focus to the
    /// surface under the cursor and sends that surface the motion.
    fn route_pointer(&mut self, time: InputTime) {
        let pointer = self.seat.get_pointer().expect("seat has a pointer");
        let target = self.cursor_target();
        let focus = target.as_ref().and_then(Target::surface);
        let location = target.map_or(pointer.current_location(), |target| target.location);
        pointer.motion(
            self,
            focus,
            &MotionEvent {
                location,
                serial: SERIAL_COUNTER.next_serial(),
                time,
            },
        );
        pointer.frame(self);
    }

    /// Things can change under a still cursor: a menu opens, a window closes.
    /// Re-route the pointer when what is under it is no longer what has
    /// pointer focus.
    fn refresh_pointer_focus(&mut self) {
        let pointer = self.seat.get_pointer().expect("seat has a pointer");
        if pointer.is_grabbed() || !self.pressed_buttons.is_empty() {
            return;
        }
        let under = self
            .cursor_target()
            .and_then(|target| target.surface())
            .map(|(surface, _)| surface);
        if under != pointer.current_focus() {
            self.route_pointer(InputTime::now());
        }
    }
}
