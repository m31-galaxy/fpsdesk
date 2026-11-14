//! Input routing.
//!
//! For now this is a plain 2D desktop: the pointer goes to whatever surface is
//! under it, a click focuses a window, and every key goes to the focused
//! window. The walk/interact mode machine replaces this in M3 and M4.

use smithay::{
    backend::input::{
        AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, InputBackend, InputEvent,
        KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent,
    },
    input::{
        keyboard::FilterResult,
        pointer::{AxisFrame, ButtonEvent, MotionEvent},
    },
    utils::SERIAL_COUNTER,
};

use crate::state::State;

/// Scroll distance, in logical pixels, for one notch of a mouse wheel.
const WHEEL_NOTCH_PIXELS: f64 = 15.0;

impl State {
    pub fn process_input_event<I: InputBackend>(&mut self, event: InputEvent<I>) {
        match event {
            InputEvent::Keyboard { event, .. } => self.on_key::<I>(event),
            InputEvent::PointerMotionAbsolute { event, .. } => self.on_pointer_absolute::<I>(event),
            InputEvent::PointerButton { event, .. } => self.on_pointer_button::<I>(event),
            InputEvent::PointerAxis { event, .. } => self.on_pointer_axis::<I>(event),
            _ => {}
        }
    }

    fn on_key<I: InputBackend>(&mut self, event: I::KeyboardKeyEvent) {
        let keyboard = self.seat.get_keyboard().expect("seat has a keyboard");
        keyboard.input::<(), _>(
            self,
            event.key_code(),
            event.state(),
            SERIAL_COUNTER.next_serial(),
            event.time(),
            |_, _, _| FilterResult::Forward,
        );
    }

    fn on_pointer_absolute<I: InputBackend>(&mut self, event: I::PointerMotionAbsoluteEvent) {
        let Some(output_geo) = self
            .space
            .outputs()
            .next()
            .and_then(|output| self.space.output_geometry(output))
        else {
            return;
        };
        let location = event.position_transformed(output_geo.size) + output_geo.loc.to_f64();

        let pointer = self.seat.get_pointer().expect("seat has a pointer");
        let under = self.surface_under(location);
        pointer.motion(
            self,
            under,
            &MotionEvent {
                location,
                serial: SERIAL_COUNTER.next_serial(),
                time: event.time(),
            },
        );
        pointer.frame(self);
    }

    fn on_pointer_button<I: InputBackend>(&mut self, event: I::PointerButtonEvent) {
        let pointer = self.seat.get_pointer().expect("seat has a pointer");

        // Click to focus, unless a client holds a grab (a drag in progress).
        if event.state() == ButtonState::Pressed && !pointer.is_grabbed() {
            let window = self
                .space
                .element_under(pointer.current_location())
                .map(|(window, _)| window.clone());
            self.focus_window(window.as_ref());
        }

        pointer.button(
            self,
            &ButtonEvent {
                button: event.button_code(),
                state: event.state(),
                serial: SERIAL_COUNTER.next_serial(),
                time: event.time(),
            },
        );
        pointer.frame(self);
    }

    fn on_pointer_axis<I: InputBackend>(&mut self, event: I::PointerAxisEvent) {
        let source = event.source();
        let mut frame = AxisFrame::new(event.time()).source(source);

        for axis in [Axis::Horizontal, Axis::Vertical] {
            // Wheels report notches in 120ths; touchpads report pixels directly.
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

        let pointer = self.seat.get_pointer().expect("seat has a pointer");
        pointer.axis(self, frame);
        pointer.frame(self);
    }
}
