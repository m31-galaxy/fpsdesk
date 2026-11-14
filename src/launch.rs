//! Starting client programs.

use std::{ffi::OsStr, process::Command};

use tracing::{info, warn};

/// Run `command` through `sh -c` as a client of the Wayland socket `socket`.
pub fn spawn_client(command: &str, socket: &OsStr) {
    let result = Command::new("sh")
        .arg("-c")
        .arg(command)
        .env("WAYLAND_DISPLAY", socket)
        // Without this, toolkits can quietly fall back to the host's X server
        // and open their window outside FPSDesk.
        .env_remove("DISPLAY")
        .spawn();
    match result {
        Ok(mut child) => {
            info!(command, pid = child.id(), "spawned client");
            // Reap the child when it exits so it does not linger as a zombie.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(err) => warn!(command, %err, "failed to spawn client"),
    }
}
