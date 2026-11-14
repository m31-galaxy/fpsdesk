//! Input routing.
//!
//! Every key goes to the focused window, which is the newest one. The pointer
//! is not routed anywhere yet: a screen position only means something to a
//! window once it can be traced through the camera onto a panel, which is the
//! ray picking that M3 and M4 add along with the walk/interact modes.

use smithay::{
    backend::input::{Event, InputBackend, InputEvent, KeyboardKeyEvent},
    input::keyboard::FilterResult,
    utils::SERIAL_COUNTER,
};

use crate::state::State;

impl State {
    pub fn process_input_event<I: InputBackend>(&mut self, event: InputEvent<I>) {
        if let InputEvent::Keyboard { event, .. } = event {
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
    }
}
