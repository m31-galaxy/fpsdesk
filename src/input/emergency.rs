//! Key bindings that must always work, in both control modes, whatever a
//! client has focus: on a TTY there is no close button, so without these a
//! stuck compositor would lock the user out of the machine.
//!
//! - Ctrl+Alt+F1..F12 switches to that virtual console.
//! - Ctrl+Alt+Backspace quits.

use smithay::input::keyboard::{Keysym, ModifiersState};
use tracing::{info, warn};

use crate::state::State;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Emergency {
    SwitchVt(i32),
    Quit,
}

/// Whether the key press `sym` (with `modifiers` held) is one of the emergency
/// bindings. `sym` is the key's keysym as modified by the held modifiers.
pub fn detect(sym: Keysym, modifiers: &ModifiersState) -> Option<Emergency> {
    if !(modifiers.ctrl && modifiers.alt) {
        return None;
    }
    let raw = sym.raw();
    // With the usual keymaps Ctrl+Alt+Fn already comes through as the
    // XF86Switch_VT_n keysyms; accept plain Fn too, in case a keymap does not
    // do that translation.
    let vt = match raw {
        r if (Keysym::XF86_Switch_VT_1.raw()..=Keysym::XF86_Switch_VT_12.raw()).contains(&r) => {
            r - Keysym::XF86_Switch_VT_1.raw() + 1
        }
        r if (Keysym::F1.raw()..=Keysym::F12.raw()).contains(&r) => r - Keysym::F1.raw() + 1,
        _ => 0,
    };
    if vt != 0 {
        return Some(Emergency::SwitchVt(vt as i32));
    }
    (sym == Keysym::BackSpace || sym == Keysym::Terminate_Server).then_some(Emergency::Quit)
}

impl State {
    /// Carry out an emergency binding.
    pub fn emergency(&mut self, action: Emergency) {
        match action {
            Emergency::Quit => {
                info!("Ctrl+Alt+Backspace: quitting");
                self.loop_signal.stop();
            }
            Emergency::SwitchVt(vt) => {
                if let Err(err) = self.backend.switch_vt(vt) {
                    warn!(vt, %err, "could not switch virtual console");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mods(ctrl: bool, alt: bool) -> ModifiersState {
        ModifiersState {
            ctrl,
            alt,
            ..Default::default()
        }
    }

    #[test]
    fn switches_vt_with_ctrl_alt_function_keys() {
        assert_eq!(
            detect(Keysym::XF86_Switch_VT_1, &mods(true, true)),
            Some(Emergency::SwitchVt(1))
        );
        assert_eq!(
            detect(Keysym::XF86_Switch_VT_12, &mods(true, true)),
            Some(Emergency::SwitchVt(12))
        );
        assert_eq!(
            detect(Keysym::F3, &mods(true, true)),
            Some(Emergency::SwitchVt(3))
        );
    }

    #[test]
    fn quits_with_ctrl_alt_backspace() {
        assert_eq!(
            detect(Keysym::BackSpace, &mods(true, true)),
            Some(Emergency::Quit)
        );
    }

    #[test]
    fn needs_both_modifiers() {
        assert_eq!(detect(Keysym::F1, &mods(true, false)), None);
        assert_eq!(detect(Keysym::F1, &mods(false, true)), None);
        assert_eq!(detect(Keysym::BackSpace, &mods(false, false)), None);
        assert_eq!(detect(Keysym::a, &mods(true, true)), None);
    }
}
