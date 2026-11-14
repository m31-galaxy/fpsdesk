//! FPSDesk: a Wayland compositor navigable in first-person 3D.
//!
//! See `docs/PLAN.md` for the design. At this stage (M1) it is an ordinary flat
//! compositor running nested in a window.

mod backend;
mod handlers;
mod input;
mod state;

use std::{error::Error, process::Command, time::Duration};

use smithay::{
    backend::renderer::gles::GlesRenderer,
    reexports::{
        calloop::{
            EventLoop,
            timer::{TimeoutAction, Timer},
        },
        wayland_server::Display,
    },
};
use tracing::{error, info, warn};

use crate::state::{Smoke, State};

const USAGE: &str = "\
Usage: fpsdesk [OPTIONS]

Options:
  -c, --command <CMD>  Run CMD (through sh) as a client once started; repeatable
      --smoke          Self-test: exit 0 after 60 frames, or exit 1 after 10 s.
                       With -c, frames only count once a client window is shown
  -h, --help           Show this help";

/// How long a `--smoke` run may take before it counts as failed.
const SMOKE_TIMEOUT: Duration = Duration::from_secs(10);

struct Args {
    smoke: bool,
    commands: Vec<String>,
}

fn parse_args() -> Result<Args, String> {
    let mut parsed = Args {
        smoke: false,
        commands: Vec::new(),
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--smoke" => parsed.smoke = true,
            "-c" | "--command" => {
                let command = args.next().ok_or(format!("{arg} needs a command"))?;
                parsed.commands.push(command);
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(parsed)
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = parse_args().map_err(|err| format!("{err}\n\n{USAGE}"))?;
    init_logging();

    let mut event_loop: EventLoop<State> = EventLoop::try_new()?;
    let display: Display<State> = Display::new()?;

    let (backend, winit) = smithay::backend::winit::init::<GlesRenderer>()?;
    let mut state = State::new(&mut event_loop, display, backend)?;
    backend::winit::init(&mut event_loop, &mut state, winit)?;

    info!(socket = ?state.socket_name, "listening for Wayland clients");

    if args.smoke {
        state.smoke = Some(Smoke {
            needs_window: !args.commands.is_empty(),
            frames: 0,
            passed: false,
        });
        event_loop
            .handle()
            .insert_source(Timer::from_duration(SMOKE_TIMEOUT), |_, _, state| {
                error!("smoke test timed out");
                state.loop_signal.stop();
                TimeoutAction::Drop
            })?;
    }

    for command in &args.commands {
        spawn_client(command, &state);
    }

    event_loop.run(None, &mut state, |_| {})?;

    match state.smoke {
        Some(Smoke { passed: false, .. }) => Err("smoke test failed".into()),
        _ => Ok(()),
    }
}

/// Start a client program connected to our Wayland socket.
fn spawn_client(command: &str, state: &State) {
    let result = Command::new("sh")
        .arg("-c")
        .arg(command)
        .env("WAYLAND_DISPLAY", &state.socket_name)
        // Without this, toolkits can quietly fall back to the host's X server
        // and open their window outside FPSDesk.
        .env_remove("DISPLAY")
        .spawn();
    match result {
        Ok(child) => info!(command, pid = child.id(), "spawned client"),
        Err(err) => warn!(command, %err, "failed to spawn client"),
    }
}

fn init_logging() {
    // Smithay logs every EGL and GL extension at info level; keep it to warnings
    // unless RUST_LOG says otherwise.
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,smithay=warn"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}
