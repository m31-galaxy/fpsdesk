use smithay::{
    desktop::{
        PopupKeyboardGrab, PopupKind, PopupPointerGrab, PopupUngrabStrategy, Window,
        find_popup_root_surface, get_popup_toplevel_coords,
    },
    input::{Seat, pointer::Focus},
    reexports::wayland_server::protocol::{wl_seat::WlSeat, wl_surface::WlSurface},
    utils::{Rectangle, Serial},
    wayland::shell::xdg::{
        PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
    },
};
use tracing::info;

use crate::state::State;

/// How far past its window's edges a popup may reach, in logical pixels,
/// before it is asked to flip or slide back.
const POPUP_MARGIN: i32 = 1000;

impl XdgShellHandler for State {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        self.add_window(Window::new_wayland_window(surface));
        info!(windows = self.windows.len(), "new window");
    }

    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        self.unconstrain_popup(&surface);
        if let Err(err) = self.popups.track_popup(PopupKind::Xdg(surface)) {
            tracing::warn!(%err, "failed to track popup");
        }
    }

    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        surface.with_pending_state(|state| {
            state.geometry = positioner.get_geometry();
            state.positioner = positioner;
        });
        self.unconstrain_popup(&surface);
        surface.send_repositioned(token);
    }

    /// A menu asks to grab input: while it is open, keys go to it, and a
    /// click anywhere outside its application dismisses it.
    ///
    /// Interactive move and resize requests are left at their no-op defaults:
    /// 2D window dragging is replaced by 3D placement in M5.
    fn grab(&mut self, surface: PopupSurface, seat: WlSeat, serial: Serial) {
        let Some(seat) = Seat::<State>::from_resource(&seat) else {
            return;
        };
        let kind = PopupKind::Xdg(surface);
        let Ok(root) = find_popup_root_surface(&kind) else {
            return;
        };
        if self.window_for_toplevel(&root).is_none() {
            return;
        }
        let Ok(mut grab) = self.popups.grab_popup(root, kind, &seat, serial) else {
            return;
        };

        // A grab is only granted in response to the input that asked for it;
        // if something else has grabbed the device since, refuse.
        if let Some(keyboard) = seat.get_keyboard() {
            if keyboard.is_grabbed()
                && !(keyboard.has_grab(serial)
                    || keyboard.has_grab(grab.previous_serial().unwrap_or(serial)))
            {
                grab.ungrab(PopupUngrabStrategy::All);
                return;
            }
            keyboard.set_focus(self, grab.current_grab(), serial);
            keyboard.set_grab(self, PopupKeyboardGrab::new(&grab), serial);
        }
        if let Some(pointer) = seat.get_pointer() {
            if pointer.is_grabbed()
                && !(pointer.has_grab(serial)
                    || pointer.has_grab(grab.previous_serial().unwrap_or_else(|| grab.serial())))
            {
                grab.ungrab(PopupUngrabStrategy::All);
                return;
            }
            pointer.set_grab(self, PopupPointerGrab::new(&grab), serial, Focus::Keep);
        }
    }
}

/// Send the initial configure that xdg-shell requires in response to a
/// surface's first commit, before the client is allowed to attach a buffer.
pub fn handle_commit(state: &mut State, surface: &WlSurface) {
    if let Some(toplevel) = state
        .window_for_toplevel(surface)
        .and_then(|window| window.toplevel())
        && !toplevel.is_initial_configure_sent()
    {
        toplevel.send_configure();
    }

    state.popups.commit(surface);
    if let Some(PopupKind::Xdg(popup)) = state.popups.find_popup(surface)
        && !popup.is_initial_configure_sent()
    {
        // The initial configure is always allowed, so this cannot fail.
        popup
            .send_configure()
            .expect("initial popup configure failed");
    }
}

impl State {
    /// Settle where a popup goes.
    ///
    /// On a flat desktop a popup is kept from running off the screen. Here
    /// there is no screen edge: a popup is drawn in its window's plane and may
    /// hang past the window's edges. So the only limit is a generous margin
    /// around the window, to stop a huge menu reaching the ground.
    fn unconstrain_popup(&self, popup: &PopupSurface) {
        let kind = PopupKind::Xdg(popup.clone());
        let Ok(root) = find_popup_root_surface(&kind) else {
            return;
        };
        let Some(window) = self.window_for_toplevel(&root) else {
            return;
        };

        // The allowed area relative to the window's top-left corner...
        let size = window.geometry().size;
        let mut target = Rectangle::new(
            (-POPUP_MARGIN, -POPUP_MARGIN).into(),
            (size.w + 2 * POPUP_MARGIN, size.h + 2 * POPUP_MARGIN).into(),
        );
        // ...and then relative to the popup's parent, as the positioner wants.
        target.loc -= get_popup_toplevel_coords(&kind);

        popup.with_pending_state(|state| {
            state.geometry = state.positioner.get_unconstrained_geometry(target);
        });
    }
}
