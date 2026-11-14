//! FPSDesk: a Wayland compositor navigable in first-person 3D.
//!
//! See `docs/PLAN.md` for the design. Runs nested in a window on a host session.

mod backend;
mod frame_stats;
mod handlers;
mod input;
mod render;
mod script;
mod shooting;
mod state;
mod world;

use std::{error::Error, path::PathBuf, process::Command, time::Duration};

use glam::Vec3;
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

use crate::{
    script::Script,
    state::{SMOKE_FRAMES, Smoke, State},
    world::{Camera, EYE_HEIGHT},
};

const USAGE: &str = "\
Usage: fpsdesk [OPTIONS]

Options:
  -c, --command <CMD>      Run CMD (through sh) as a client once started; repeatable
      --smoke              Self-test: exit 0 after 60 frames, or exit 1 after 10 s.
                           With -c, frames only count once a client window is shown
      --smoke-frames <N>   With --smoke: frames to render after the script ends
                           before passing and taking the screenshot (default 60)
      --screenshot <PATH>  With --smoke: save the last frame to PATH as a PNG
      --script <STEPS>     Play scripted input, one step a frame; see src/script.rs
      --camera <X,Z,YAW,PITCH>
                           Start X metres right of and Z metres behind the origin,
                           turned YAW degrees left and looking PITCH degrees up
  -h, --help               Show this help

Controls:
  Walk mode      W/A/S/D or arrows move, Shift runs, the mouse looks around.
                 Left click or Enter uses the window under the crosshair.
                 Right click shoots it, asking it to close.
  Interact mode  Keys go to the focused window.
  Either         Ctrl+Escape (or Super) switches between the two modes.";

/// How long a `--smoke` run may take before it counts as failed.
const SMOKE_TIMEOUT: Duration = Duration::from_secs(10);

struct Args {
    smoke: bool,
    smoke_frames: u32,
    screenshot: Option<PathBuf>,
    camera: Option<[f32; 4]>,
    script: Option<String>,
    commands: Vec<String>,
}

fn parse_args() -> Result<Args, String> {
    let mut parsed = Args {
        smoke: false,
        smoke_frames: SMOKE_FRAMES,
        screenshot: None,
        camera: None,
        script: None,
        commands: Vec::new(),
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--smoke" => parsed.smoke = true,
            "--smoke-frames" => {
                let frames = args.next().and_then(|n| n.parse().ok());
                parsed.smoke_frames = frames.ok_or("--smoke-frames needs a number")?;
            }
            "--screenshot" => {
                let path = args.next().ok_or("--screenshot needs a path")?;
                parsed.screenshot = Some(path.into());
            }
            "--script" => {
                parsed.script = Some(args.next().ok_or("--script needs a list of steps")?);
            }
            "--camera" => {
                let pose = args.next().ok_or("--camera needs X,Z,YAW,PITCH")?;
                let numbers: Result<Vec<f32>, _> = pose.split(',').map(str::parse).collect();
                let pose = numbers.ok().and_then(|numbers| numbers.try_into().ok());
                parsed.camera = Some(pose.ok_or("--camera needs four numbers: X,Z,YAW,PITCH")?);
            }
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
    if parsed.screenshot.is_some() && !parsed.smoke {
        return Err("--screenshot only works together with --smoke".into());
    }
    Ok(parsed)
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = match parse_args() {
        Ok(args) => args,
        Err(err) => {
            eprintln!("fpsdesk: {err}\n\n{USAGE}");
            std::process::exit(2);
        }
    };
    init_logging();

    let mut event_loop: EventLoop<State> = EventLoop::try_new()?;
    let display: Display<State> = Display::new()?;

    let (backend, winit) = smithay::backend::winit::init::<GlesRenderer>()?;
    let mut state = State::new(&mut event_loop, display, backend)?;
    backend::winit::init(&mut event_loop, &mut state, winit)?;

    if let Some([x, z, yaw, pitch]) = args.camera {
        state.camera = Camera::new(
            Vec3::new(x, EYE_HEIGHT, z),
            yaw.to_radians(),
            pitch.to_radians(),
        );
    }

    if let Some(steps) = &args.script {
        // With clients to start, hold the script until one has a window up.
        state.script = Some(Script::parse(steps, !args.commands.is_empty())?);
    }

    info!(socket = ?state.socket_name, "listening for Wayland clients");
    info!("walk mode: WASD and mouse move you; Ctrl+Escape releases the mouse");

    if args.smoke {
        state.smoke = Some(Smoke {
            needs_window: !args.commands.is_empty(),
            frames: 0,
            passed: false,
            frames_needed: args.smoke_frames,
            screenshot: args.screenshot.clone(),
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
