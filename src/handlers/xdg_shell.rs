use smithay::{
    desktop::{PopupKind, Window, find_popup_root_surface, get_popup_toplevel_coords},
    reexports::wayland_server::protocol::{wl_seat::WlSeat, wl_surface::WlSurface},
    utils::Serial,
    wayland::shell::xdg::{
        PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
    },
};
use tracing::info;

use crate::state::State;

/// New windows are offset from the previous one by this much, wrapping after
/// `CASCADE_STEPS`, so that they do not land exactly on top of each other.
const CASCADE_OFFSET: i32 = 32;
const CASCADE_STEPS: i32 = 8;

impl XdgShellHandler for State {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        let step = self.space.elements().count() as i32 % CASCADE_STEPS;
        let offset = CASCADE_OFFSET * step;

        let window = Window::new_wayland_window(surface);
        self.space
            .map_element(window.clone(), (offset, offset), false);
        self.focus_window(Some(&window));
        info!(windows = self.space.elements().count(), "new window");
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

    // Popup grabs (click outside a menu to dismiss it) come with interact mode
    // in M4. Interactive move and resize requests are left at their no-op
    // defaults: 2D window dragging is replaced by 3D placement in M5.
    fn grab(&mut self, _surface: PopupSurface, _seat: WlSeat, _serial: Serial) {}
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
    /// Adjust a popup's position so that it stays within the output.
    fn unconstrain_popup(&self, popup: &PopupSurface) {
        let kind = PopupKind::Xdg(popup.clone());
        let Ok(root) = find_popup_root_surface(&kind) else {
            return;
        };
        let Some(window) = self.window_for_toplevel(&root) else {
            return;
        };
        let Some(output_geo) = self
            .space
            .outputs()
            .next()
            .and_then(|output| self.space.output_geometry(output))
        else {
            return;
        };
        let Some(window_geo) = self.space.element_geometry(window) else {
            return;
        };

        // The positioner works relative to the popup's parent, so express the
        // output rectangle in those coordinates.
        let mut target = output_geo;
        target.loc -= get_popup_toplevel_coords(&kind);
        target.loc -= window_geo.loc;

        popup.with_pending_state(|state| {
            state.geometry = state.positioner.get_unconstrained_geometry(target);
        });
    }
}
