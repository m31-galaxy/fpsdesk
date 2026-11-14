//! Backends connect the compositor to a display and input devices.
//!
//! - `winit`: nested in a window on a host session.
//! - `tty`: DRM/KMS and libinput on a virtual console, as a real session.
//!
//! The per-frame work both share is in `frame.rs`.

mod relative_pointer;
pub mod tty;
pub mod winit;

use smithay::backend::{renderer::gles::GlesRenderer, winit::WinitGraphicsBackend};

/// The backend that owns the output, and with it the renderer.
pub enum Backend {
    Winit(Box<WinitGraphicsBackend<GlesRenderer>>),
    Tty(Box<tty::Tty>),
}

impl Backend {
    /// The GL renderer, whichever backend owns it.
    pub fn renderer(&mut self) -> &mut GlesRenderer {
        match self {
            Backend::Winit(backend) => backend.renderer(),
            Backend::Tty(tty) => tty.renderer(),
        }
    }

    /// The nested backend's window. Only the nested backend's own code calls
    /// this.
    pub fn winit(&mut self) -> &mut WinitGraphicsBackend<GlesRenderer> {
        match self {
            Backend::Winit(backend) => backend,
            Backend::Tty(_) => unreachable!("not running nested"),
        }
    }

    /// The TTY backend. Only the TTY backend's own code calls this.
    pub fn tty(&mut self) -> &mut tty::Tty {
        match self {
            Backend::Tty(tty) => tty,
            Backend::Winit(_) => unreachable!("not running on a TTY"),
        }
    }

    /// Switch to virtual console `vt`. Does nothing when nested.
    pub fn switch_vt(&mut self, vt: i32) -> Result<(), Box<dyn std::error::Error>> {
        match self {
            Backend::Winit(_) => Ok(()),
            Backend::Tty(tty) => tty.switch_vt(vt),
        }
    }

    /// Give the display back before exiting, so that the console is restored.
    pub fn shutdown(&mut self) {
        if let Backend::Tty(tty) = self {
            tty.shutdown();
        }
    }
}
