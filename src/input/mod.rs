//! Input routing and the control modes.
//!
//! In **walk** mode the keyboard and mouse move the viewer and no client has
//! keyboard focus. In **interact** mode keys go to the focused window. The
//! pointer is not routed to clients yet; that is M4.
//!
//! Bindings:
//! - Walk: W/A/S/D or arrows move, Shift runs, the mouse looks. Left click or
//!   Enter on the window under the crosshair focuses it and enters interact.
//! - Either mode: Super, or Ctrl+Escape, switches to the other mode. (Under a
//!   host desktop Super usually never reaches us, hence the second binding.)

pub mod controls;

use glam::Vec2;
use smithay::{
    backend::input::{
        AbsolutePositionEvent, ButtonState, Event, InputBackend, InputEvent, KeyState,
        KeyboardKeyEvent, MouseButton, PointerButtonEvent, PointerMotionEvent,
    },
    desktop::Window,
    input::keyboard::{FilterResult, Keysym},
    utils::SERIAL_COUNTER,
};
use tracing::info;

use crate::{
    state::{Highlight, State},
    world,
};
use controls::{Mode, key, look_delta};

/// What a key press that we kept for ourselves asks for.
enum KeyAction {
    ToggleMode,
    /// A key that means something in walk mode, or nothing at all.
    Walk,
}

impl State {
    pub fn process_input_event<I: InputBackend>(&mut self, event: InputEvent<I>) {
        match event {
            InputEvent::Keyboard { event, .. } => self.on_key::<I>(event),
            InputEvent::PointerMotion { event, .. } => {
                self.look(Vec2::new(event.delta_x() as f32, event.delta_y() as f32));
            }
            InputEvent::PointerMotionAbsolute { event, .. } => self.on_pointer_absolute::<I>(event),
            InputEvent::PointerButton { event, .. } => {
                let left_press = event.button() == Some(MouseButton::Left)
                    && event.state() == ButtonState::Pressed;
                if left_press && self.mode == Mode::Walk {
                    self.use_hovered();
                }
            }
            _ => {}
        }
    }

    fn on_key<I: InputBackend>(&mut self, event: I::KeyboardKeyEvent) {
        let keycode = event.key_code();
        // xkb key codes are evdev codes offset by 8.
        let evdev_code = keycode.raw().saturating_sub(8);
        let pressed = event.state() == KeyState::Pressed;

        let keyboard = self.seat.get_keyboard().expect("seat has a keyboard");
        let action = keyboard.input(
            self,
            keycode,
            event.state(),
            SERIAL_COUNTER.next_serial(),
            event.time(),
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
            Some(KeyAction::ToggleMode) => match self.mode {
                Mode::Walk => self.enter_interact(self.hovered.clone()),
                Mode::Interact => self.enter_walk(),
            },
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

    /// Absolute pointer positions are all a nested window's host gives us by
    /// default. If the backend has no better source of relative motion, turn
    /// the differences between them into mouselook; the view then stops
    /// turning when the pointer reaches the edge of the window.
    fn on_pointer_absolute<I: InputBackend>(&mut self, event: I::PointerMotionAbsoluteEvent) {
        let Some(size) = self
            .outputs
            .first()
            .and_then(|output| output.current_mode())
            .map(|mode| mode.size)
        else {
            return;
        };
        let scale = self
            .outputs
            .first()
            .map_or(1.0, |output| output.current_scale().fractional_scale());
        let logical = size.to_f64().to_logical(scale).to_i32_round();
        let position = event.position_transformed(logical);
        let position = Vec2::new(position.x as f32, position.y as f32);

        if let Some(last) = self.last_pointer_position.replace(position)
            && !self.has_relative_pointer
        {
            self.look(position - last);
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

    /// Focus the window under the crosshair, if there is one.
    fn use_hovered(&mut self) {
        if let Some(window) = self.hovered.clone() {
            self.enter_interact(Some(window));
        }
    }

    pub fn enter_walk(&mut self) {
        info!("walk mode");
        self.mode = Mode::Walk;
        self.look_motion = Vec2::ZERO;
        self.focus_window(None);
    }

    pub fn enter_interact(&mut self, window: Option<Window>) {
        info!(focused = window.is_some(), "interact mode");
        self.mode = Mode::Interact;
        self.move_keys.release_all();
        self.look_motion = Vec2::ZERO;
        self.focus_window(window.as_ref());
    }

    /// Advance the world by `dt` seconds: apply movement and mouselook, then
    /// work out which window the crosshair is on. `aspect` is the view's
    /// width over its height.
    pub fn advance(&mut self, dt: f32, aspect: f32) {
        if self.mode == Mode::Walk {
            let (yaw, pitch) = look_delta(std::mem::take(&mut self.look_motion));
            self.camera.turn(yaw, pitch);
            self.camera.walk(self.move_keys.step(dt));
        }

        // The crosshair is the centre of the screen.
        let ray = self.camera.ray_through(Vec2::ZERO, aspect);
        // Only windows that have been drawn have a panel of the right size.
        let shown = || self.windows.iter().filter(|entry| entry.texture.is_some());
        let hit = world::pick(&ray, shown().map(|entry| &entry.panel));
        self.hovered = hit.and_then(|(index, _)| shown().nth(index).map(|e| e.window.clone()));

        for entry in &mut self.windows {
            let is = |window: &Option<Window>| window.as_ref() == Some(&entry.window);
            entry.highlight = match self.mode {
                Mode::Walk if is(&self.hovered) => Highlight::Hovered,
                Mode::Interact if is(&self.focused) => Highlight::Focused,
                _ => Highlight::None,
            };
        }
    }
}
