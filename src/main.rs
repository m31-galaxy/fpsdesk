//! FPSDesk: a Wayland compositor navigable in first-person 3D.
//!
//! See `docs/PLAN.md` for the design. Runs nested in a window on a host session,
//! or as a real session on a virtual console.

mod backend;
mod frame;
mod frame_stats;
mod handlers;
mod input;
mod launch;
mod prompt;
mod render;
mod script;
mod shooting;
mod state;
mod text;
mod world;

use std::{error::Error, path::PathBuf, time::Duration};

use glam::Vec3;
use smithay::reexports::{
    calloop::{
        EventLoop,
        timer::{TimeoutAction, Timer},
    },
    wayland_server::Display,
};
use tracing::{error, info};

use crate::{
    backend::{Backend, tty},
    launch::spawn_client,
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
      --exit-after <SECS>  Quit after SECS seconds whatever happens. A safety net
                           for trying the tty backend: it gets you back to the
                           console even if the keyboard does not respond
      --script <STEPS>     Play scripted input, one step a frame; see src/script.rs
      --camera <X,Z,YAW,PITCH>
                           Start X metres right of and Z metres behind the origin,
                           turned YAW degrees left and looking PITCH degrees up
      --backend <nested|tty>
                           nested: a window on the running desktop; tty: a real
                           session on a virtual console. Default: nested if
                           WAYLAND_DISPLAY or DISPLAY is set, else tty
      --scale <FACTOR>     tty only: output scale, such as 2 for a HiDPI panel
                           (default 1)
  -h, --help               Show this help

Controls:
  Walk mode      W/A/S/D or arrows move, Shift runs, the mouse looks around.
                 Left click or Enter uses the window under the crosshair.
                 ` or / opens the launcher: type a command, Enter runs it,
                 Up recalls the previous one, Escape cancels.
                 Right click shoots the window under the crosshair; three hits
                 close it.
  Interact mode  Keys go to the focused window.
  Either         Ctrl+Escape (or Super) switches between the two modes.
  tty backend    Ctrl+Alt+F1..F12 switches virtual console;
                 Ctrl+Alt+Backspace quits.";

/// How long a `--smoke` run may take before it counts as failed.
const SMOKE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, PartialEq, Eq)]
enum BackendKind {
    Nested,
    Tty,
}

struct Args {
    backend: Option<BackendKind>,
    scale: f64,
    smoke: bool,
    smoke_frames: u32,
    exit_after: Option<u64>,
    screenshot: Option<PathBuf>,
    camera: Option<[f32; 4]>,
    script: Option<String>,
    commands: Vec<String>,
}

fn parse_args() -> Result<Args, String> {
    let mut parsed = Args {
        backend: None,
        scale: 1.0,
        smoke: false,
        smoke_frames: SMOKE_FRAMES,
        exit_after: None,
        screenshot: None,
        camera: None,
        script: None,
        commands: Vec::new(),
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--smoke" => parsed.smoke = true,
            "--backend" => {
                parsed.backend = Some(match args.next().as_deref() {
                    Some("nested") => BackendKind::Nested,
                    Some("tty") => BackendKind::Tty,
                    _ => return Err("--backend needs `nested` or `tty`".into()),
                });
            }
            "--scale" => {
                let scale = args.next().and_then(|scale| scale.parse::<f64>().ok());
                parsed.scale = scale
                    .filter(|scale| *scale > 0.0)
                    .ok_or("--scale needs a positive number")?;
            }
            "--exit-after" => {
                let seconds = args.next().and_then(|n| n.parse().ok());
                parsed.exit_after = Some(seconds.ok_or("--exit-after needs a number of seconds")?);
            }
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

    let mut state = match args.backend.unwrap_or_else(default_backend) {
        BackendKind::Nested => {
            let (backend, winit) = smithay::backend::winit::init()?;
            let mut state =
                State::new(&mut event_loop, display, Backend::Winit(Box::new(backend)))?;
            backend::winit::init(&mut event_loop, &mut state, winit)?;
            state
        }
        BackendKind::Tty => {
            let (backend, events) = tty::Tty::new()?;
            let backend = Backend::Tty(Box::new(backend));
            let mut state = State::new(&mut event_loop, display, backend)?;
            tty::init(&mut event_loop, &mut state, events, args.scale)?;
            state
        }
    };

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

    if let Some(seconds) = args.exit_after {
        let timer = Timer::from_duration(Duration::from_secs(seconds));
        event_loop
            .handle()
            .insert_source(timer, move |_, _, state| {
                info!(seconds, "--exit-after: time is up, quitting");
                state.loop_signal.stop();
                TimeoutAction::Drop
            })?;
    }

    for command in &args.commands {
        spawn_client(command, &state.socket_name);
    }

    let result = event_loop.run(None, &mut state, |_| {});
    // Give the display back before anything else is torn down, even if the
    // loop failed.
    state.backend.shutdown();
    result?;

    match state.smoke {
        Some(Smoke { passed: false, .. }) => Err("smoke test failed".into()),
        _ => Ok(()),
    }
}

/// Nested if there is a desktop to be nested in, else a real session.
fn default_backend() -> BackendKind {
    let has = |name| std::env::var_os(name).is_some_and(|value| !value.is_empty());
    if has("WAYLAND_DISPLAY") || has("DISPLAY") {
        BackendKind::Nested
    } else {
        BackendKind::Tty
    }
}

fn init_logging() {
    // Smithay logs every EGL and GL extension at info level; keep it to warnings
    // unless RUST_LOG says otherwise.
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,smithay=warn"));
    // To stderr, alongside what clients print there, so that redirecting it
    // captures the whole story; stdout is hidden behind the picture on a TTY.
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
}
