//! FPSDesk entry point.
//!
//! For now this is only a toolchain smoke test: it opens a nested window on the
//! host compositor through Smithay's winit backend, reports which GL driver it
//! got, and clears the window every frame. See `docs/PLAN.md` for what replaces it.

use std::ffi::{CStr, c_char};

use smithay::{
    backend::{
        renderer::{
            Color32F, Frame, Renderer,
            gles::{GlesRenderer, ffi},
        },
        winit::{self, WinitEvent},
    },
    reexports::calloop::EventLoop,
    utils::{Rectangle, Transform},
};
use tracing::info;

/// Frames to draw before exiting when run with `--smoke`.
const SMOKE_FRAMES: u32 = 60;

const CLEAR_COLOR: Color32F = Color32F::new(0.05, 0.07, 0.12, 1.0);

struct State {
    running: bool,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_logging();

    let smoke = std::env::args().skip(1).any(|arg| arg == "--smoke");

    let mut event_loop: EventLoop<State> = EventLoop::try_new()?;
    let (mut backend, winit) = winit::init::<GlesRenderer>()?;

    let (version, renderer_name) = backend.renderer().with_context(|gl| unsafe {
        (gl_string(gl, ffi::VERSION), gl_string(gl, ffi::RENDERER))
    })?;
    info!(%version, renderer = %renderer_name, "nested backend ready");

    let mut frames: u32 = 0;
    event_loop
        .handle()
        .insert_source(winit, move |event, _, state| match event {
            WinitEvent::Redraw => {
                let size = backend.window_size();
                let damage = Rectangle::from_size(size);
                {
                    let (renderer, mut framebuffer) = backend.bind().expect("bind window surface");
                    let mut frame = renderer
                        .render(&mut framebuffer, size, Transform::Flipped180)
                        .expect("begin frame");
                    frame.clear(CLEAR_COLOR, &[damage]).expect("clear");
                    // The sync point only matters when another context reads this
                    // target; swapping the window surface below flushes on its own.
                    let _sync = frame.finish().expect("finish frame");
                }
                backend.submit(Some(&[damage])).expect("swap buffers");

                frames += 1;
                if smoke && frames >= SMOKE_FRAMES {
                    info!(frames, "smoke test passed");
                    state.running = false;
                }
                backend.window().request_redraw();
            }
            WinitEvent::CloseRequested => state.running = false,
            _ => {}
        })?;

    let mut state = State { running: true };
    let signal = event_loop.get_signal();
    event_loop.run(None, &mut state, move |state| {
        if !state.running {
            signal.stop();
        }
    })?;

    Ok(())
}

fn init_logging() {
    // Smithay logs every EGL and GL extension at info level; keep it to warnings
    // unless RUST_LOG says otherwise.
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,smithay=warn"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

/// Read one of the `glGetString` values from the current context.
///
/// # Safety
/// A GL context must be current on this thread.
unsafe fn gl_string(gl: &ffi::Gles2, name: ffi::types::GLenum) -> String {
    let ptr = unsafe { gl.GetString(name) } as *const c_char;
    if ptr.is_null() {
        return String::from("<unknown>");
    }
    unsafe { CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned()
}
