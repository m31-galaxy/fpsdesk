//! Wayland protocol handlers.
//!
//! Smithay implements the protocols; these impls are where it asks us for
//! policy. The handlers in this module and its children are adapted from
//! Smithay's `smallvil` example (MIT, see `docs/licenses/smithay-MIT.txt`).

mod compositor;
pub mod dmabuf;
mod xdg_shell;

use smithay::{
    input::{
        Seat, SeatHandler, SeatState,
        dnd::{DnDGrab, DndGrabHandler, GrabType, Source},
        pointer::{CursorImageStatus, Focus},
    },
    reexports::wayland_server::{Resource, protocol::wl_surface::WlSurface},
    utils::Serial,
    wayland::{
        output::OutputHandler,
        pointer_constraints::PointerConstraintsHandler,
        selection::{
            SelectionHandler,
            data_device::{
                DataDeviceHandler, DataDeviceState, WaylandDndGrabHandler, set_data_device_focus,
            },
        },
    },
};

use crate::state::State;

impl SeatHandler for State {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<State> {
        &mut self.seat_state
    }

    fn cursor_image(&mut self, _seat: &Seat<Self>, image: CursorImageStatus) {
        self.cursor_status = image;
    }

    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&WlSurface>) {
        // The clipboard follows keyboard focus.
        let dh = &self.display_handle;
        let client = focused.and_then(|surface| dh.get_client(surface.id()).ok());
        set_data_device_focus(dh, seat, client);
    }
}

impl PointerConstraintsHandler for State {}

impl SelectionHandler for State {
    type SelectionUserData = ();
}

impl DataDeviceHandler for State {
    fn data_device_state(&mut self) -> &mut DataDeviceState {
        &mut self.data_device_state
    }
}

impl DndGrabHandler for State {}

impl WaylandDndGrabHandler for State {
    fn dnd_requested<S: Source>(
        &mut self,
        source: S,
        _icon: Option<WlSurface>,
        seat: Seat<Self>,
        serial: Serial,
        type_: GrabType,
    ) {
        let pointer = match type_ {
            GrabType::Pointer => seat.get_pointer(),
            // No touch input yet.
            GrabType::Touch => None,
        };
        let Some((pointer, start_data)) =
            pointer.and_then(|p| p.grab_start_data().map(|data| (p, data)))
        else {
            source.cancel();
            return;
        };

        let grab = DnDGrab::new_pointer(&self.display_handle, start_data, source, seat);
        pointer.set_grab(self, grab, serial, Focus::Keep);
    }
}

impl OutputHandler for State {}

smithay::delegate_dispatch2!(State);
