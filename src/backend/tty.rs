//! TTY backend: FPSDesk as a real session on a virtual console.
//!
//! The simplest path that works: one GPU (the primary one), one output (the
//! internal panel if there is one, else the first connected connector, at its
//! preferred mode), libseat for the session, DRM/KMS with GBM buffers for the
//! display, EGL on GBM for rendering, and libinput for input.
//!
//! Frames are driven by vblank: the first frame is rendered at startup, and
//! each page flip's completion triggers the next. When the session is paused
//! (switched to another VT) DRM and libinput are paused with it and nothing
//! is drawn; on resume the mode is restored and the cycle restarted.
//!
//! The safety bindings (VT switching and quitting) are in `input/emergency.rs`.
//! Any error while rendering or flipping is logged and stops the event loop,
//! so that FPSDesk exits and gives the console back instead of hanging.

use std::{
    error::Error,
    path::PathBuf,
    time::{Duration, Instant},
};

use smithay::{
    backend::{
        allocator::{
            Fourcc,
            gbm::{GbmAllocator, GbmBufferFlags, GbmDevice},
        },
        drm::{DrmDevice, DrmDeviceFd, DrmDeviceNotifier, DrmEvent, GbmBufferedSurface},
        egl::{EGLContext, EGLDisplay},
        input::InputEvent,
        libinput::{LibinputInputBackend, LibinputSessionInterface},
        renderer::{Bind, gles::GlesRenderer},
        session::{
            Event as SessionEvent, Session,
            libseat::{LibSeatSession, LibSeatSessionNotifier},
        },
        udev::{all_gpus, primary_gpu},
    },
    output::{Mode as OutputMode, Output, PhysicalProperties, Scale, Subpixel},
    reexports::{
        calloop::{
            EventLoop,
            timer::{TimeoutAction, Timer},
        },
        drm::control::{Device as ControlDevice, ModeTypeFlags, connector, crtc},
        input::{Device as InputDevice, Libinput},
        rustix::fs::OFlags,
    },
    utils::{DeviceFd, Physical, Size, Transform},
};
use tracing::{debug, error, info, warn};

use super::Backend;
use crate::{
    frame::{self, Frame, FrameParams},
    state::State,
};

/// Pixel formats to try for the scanout buffers, in order. 8 bits a channel
/// keeps screenshots and colours simple; the plane may only offer the opaque
/// (`X`) variant of the first, which Smithay then uses.
const COLOR_FORMATS: [Fourcc; 2] = [Fourcc::Argb8888, Fourcc::Abgr8888];

/// A page flip should complete within a frame or so. If none has for this
/// long (the display was switched off, say), start the cycle again.
const STALL_TIMEOUT: Duration = Duration::from_secs(3);

type Surface = GbmBufferedSurface<GbmAllocator<DrmDeviceFd>, ()>;

/// The display we drive.
struct TtyOutput {
    output: Output,
    surface: Surface,
    size: Size<i32, Physical>,
    scale: f64,
    /// When a frame last went to the display, for spotting stalls.
    last_queued: Instant,
}

/// Everything the TTY backend owns. Fields drop in order, so the display and
/// GPU go before the session that lent them.
pub struct Tty {
    output: Option<TtyOutput>,
    renderer: GlesRenderer,
    drm: DrmDevice,
    gbm: GbmDevice<DrmDeviceFd>,
    libinput: Libinput,
    session: LibSeatSession,
    /// Whether the session is active, that is, whether we own the display
    /// right now.
    active: bool,
    frame: Frame,
}

/// The event sources that go with a `Tty`, to be put in the event loop by
/// `init`.
pub struct TtyEvents {
    session: LibSeatSessionNotifier,
    drm: DrmDeviceNotifier,
    input: LibinputInputBackend,
}

impl Tty {
    /// Open the session, the GPU and the input devices.
    pub fn new() -> Result<(Self, TtyEvents), Box<dyn Error>> {
        let (mut session, session_notifier) = LibSeatSession::new().map_err(|err| {
            format!(
                "could not open a session ({err}). Run this from a virtual console, \
                 with seatd or logind available"
            )
        })?;
        let seat = session.seat();
        info!(seat, "session opened");

        let path = gpu_path(&seat)?;
        info!(?path, "using GPU");
        let fd = session
            .open(
                &path,
                OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY | OFlags::NONBLOCK,
            )
            .map_err(|err| format!("could not open {}: {err}", path.display()))?;
        let fd = DrmDeviceFd::new(DeviceFd::from(fd));

        let (drm, drm_notifier) = DrmDevice::new(fd.clone(), true)?;
        let gbm = GbmDevice::new(fd)?;

        // Safety: the display is made from a GBM device that we keep alive
        // for as long as the renderer.
        let renderer = unsafe {
            let display = EGLDisplay::new(gbm.clone())?;
            let context = EGLContext::new(&display)?;
            GlesRenderer::new(context)?
        };

        let mut libinput = Libinput::new_with_udev::<LibinputSessionInterface<LibSeatSession>>(
            session.clone().into(),
        );
        libinput
            .udev_assign_seat(&seat)
            .map_err(|()| format!("libinput could not use seat {seat}"))?;
        let input = LibinputInputBackend::new(libinput.clone());

        let tty = Self {
            output: None,
            renderer,
            drm,
            gbm,
            libinput,
            session,
            active: true,
            frame: Frame::new(),
        };
        let events = TtyEvents {
            session: session_notifier,
            drm: drm_notifier,
            input,
        };
        Ok((tty, events))
    }

    pub fn renderer(&mut self) -> &mut GlesRenderer {
        &mut self.renderer
    }

    pub fn switch_vt(&mut self, vt: i32) -> Result<(), Box<dyn Error>> {
        info!(vt, "switching virtual console");
        self.session.change_vt(vt)?;
        Ok(())
    }

    /// Stop driving the display, so that the console comes back.
    pub fn shutdown(&mut self) {
        // Dropping the surface turns the output off and hands the display
        // back; closing the session (when we drop) restores the console.
        self.output = None;
    }

    /// Choose the connector to drive and set up the output on it.
    fn create_output(
        &mut self,
        dh: &smithay::reexports::wayland_server::DisplayHandle,
        scale: f64,
    ) -> Result<Output, Box<dyn Error>> {
        let (info, crtc) = self.pick_connector()?;
        let drm_mode = info
            .modes()
            .iter()
            .find(|mode| mode.mode_type().contains(ModeTypeFlags::PREFERRED))
            .or_else(|| info.modes().first())
            .copied()
            .ok_or("the connector has no modes")?;
        let name = connector_name(&info);
        info!(
            output = name,
            mode = ?drm_mode.size(),
            refresh = drm_mode.vrefresh(),
            ?crtc,
            "driving output"
        );

        let drm_surface = self.drm.create_surface(crtc, drm_mode, &[info.handle()])?;
        let allocator = GbmAllocator::new(
            self.gbm.clone(),
            GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT,
        );
        let render_formats = self
            .renderer
            .egl_context()
            .dmabuf_render_formats()
            .iter()
            .copied();
        let surface =
            GbmBufferedSurface::new(drm_surface, allocator, &COLOR_FORMATS, render_formats)
                .map_err(|err| format!("no usable scanout buffer format: {err}"))?;
        debug!(format = ?surface.format(), "scanout format");

        let mode = OutputMode::from(drm_mode);
        let (width_mm, height_mm) = info.size().unwrap_or((0, 0));
        let output = Output::new(
            name,
            PhysicalProperties {
                size: (width_mm as i32, height_mm as i32).into(),
                subpixel: Subpixel::Unknown,
                make: "Unknown".into(),
                model: "Unknown".into(),
                serial_number: "Unknown".into(),
            },
        );
        let _global = output.create_global::<State>(dh);
        output.change_current_state(
            Some(mode),
            Some(Transform::Normal),
            Some(Scale::Fractional(scale)),
            Some((0, 0).into()),
        );
        output.set_preferred(mode);

        self.output = Some(TtyOutput {
            output: output.clone(),
            surface,
            size: mode.size,
            scale,
            last_queued: Instant::now(),
        });
        Ok(output)
    }

    /// The connected connector to show the world on, and a CRTC for it. Takes
    /// the internal panel if there is one, and logs the others as ignored.
    fn pick_connector(&self) -> Result<(connector::Info, crtc::Handle), Box<dyn Error>> {
        let resources = self.drm.resource_handles()?;
        let mut connected: Vec<connector::Info> = resources
            .connectors()
            .iter()
            .filter_map(|handle| self.drm.get_connector(*handle, true).ok())
            .filter(|info| info.state() == connector::State::Connected)
            .collect();
        // Stable, so otherwise the kernel's order stands.
        connected.sort_by_key(|info| !is_internal(info));

        let mut chosen = None;
        for info in connected {
            if chosen.is_some() {
                info!(output = connector_name(&info), "ignoring extra monitor");
                continue;
            }
            if info.modes().is_empty() {
                warn!(output = connector_name(&info), "connected but has no modes");
                continue;
            }
            match self.crtc_for(&info, &resources) {
                Some(crtc) => chosen = Some((info, crtc)),
                None => warn!(output = connector_name(&info), "no CRTC can drive it"),
            }
        }
        chosen.ok_or_else(|| "no connected display found".into())
    }

    /// A CRTC that can drive `info`: the one it is already on if it is on one
    /// (to avoid a needless modeset), else any that its encoders allow.
    fn crtc_for(
        &self,
        info: &connector::Info,
        resources: &smithay::reexports::drm::control::ResourceHandles,
    ) -> Option<crtc::Handle> {
        let encoders: Vec<_> = info
            .current_encoder()
            .into_iter()
            .chain(info.encoders().iter().copied())
            .filter_map(|handle| self.drm.get_encoder(handle).ok())
            .collect();
        encoders
            .iter()
            .find_map(|encoder| encoder.crtc())
            .or_else(|| {
                encoders.iter().find_map(|encoder| {
                    resources
                        .filter_crtcs(encoder.possible_crtcs())
                        .first()
                        .copied()
                })
            })
    }
}

/// The DRM device to drive: `FPSDESK_DRM_DEVICE` if set, else the primary GPU.
fn gpu_path(seat: &str) -> Result<PathBuf, Box<dyn Error>> {
    if let Some(path) = std::env::var_os("FPSDESK_DRM_DEVICE") {
        return Ok(path.into());
    }
    if let Some(path) = primary_gpu(seat)? {
        return Ok(path);
    }
    all_gpus(seat)?
        .into_iter()
        .next()
        .ok_or_else(|| "no GPU found".into())
}

fn is_internal(info: &connector::Info) -> bool {
    use connector::Interface;
    matches!(
        info.interface(),
        Interface::EmbeddedDisplayPort | Interface::LVDS | Interface::DSI
    )
}

fn connector_name(info: &connector::Info) -> String {
    format!("{}-{}", info.interface().as_str(), info.interface_id())
}

/// Open the output on the display, wire the backend into the event loop and
/// start drawing.
pub fn init(
    event_loop: &mut EventLoop<State>,
    state: &mut State,
    events: TtyEvents,
    scale: f64,
) -> Result<(), Box<dyn Error>> {
    let Backend::Tty(tty) = &mut state.backend else {
        unreachable!("not the TTY backend");
    };
    let output = tty.create_output(&state.display_handle, scale)?;
    state.outputs.push(output);
    // libinput is relative by nature, so mouselook never needs the fallback
    // of differences between absolute positions.
    state.has_relative_pointer = true;

    let handle = event_loop.handle();

    handle.insert_source(events.input, |mut event, _, state| {
        if let InputEvent::DeviceAdded { device } = &mut event {
            configure_device(device);
        }
        state.process_input_event(event);
    })?;

    handle.insert_source(events.drm, |event, _, state| match event {
        DrmEvent::VBlank(_) => on_vblank(state),
        DrmEvent::Error(err) => {
            // While switched away, errors are expected noise.
            if state.backend.tty().active {
                fatal(state, format!("DRM error: {err}"));
            } else {
                debug!(%err, "DRM error while paused");
            }
        }
    })?;

    handle.insert_source(events.session, |event, &mut (), state| match event {
        SessionEvent::PauseSession => pause(state),
        SessionEvent::ActivateSession => resume(state),
    })?;

    // Page flips that never complete would otherwise hang us silently.
    handle.insert_source(Timer::from_duration(STALL_TIMEOUT), |_, _, state| {
        check_stall(state);
        TimeoutAction::ToDuration(STALL_TIMEOUT)
    })?;

    // The first frame: everything after it follows from vblank.
    handle.insert_idle(render);
    Ok(())
}

/// Enable tap-to-click on touchpads.
fn configure_device(device: &mut InputDevice) {
    if device.config_tap_finger_count() > 0 {
        let _ = device.config_tap_set_enabled(true);
        info!(device = %device.name(), "touchpad: tap to click enabled");
    }
}

/// Log a fatal error and stop, so that the console is given back.
fn fatal(state: &mut State, message: String) {
    error!("{message}; exiting");
    state.loop_signal.stop();
}

/// The last frame reached the screen: draw the next.
fn on_vblank(state: &mut State) {
    let tty = state.backend.tty();
    let Some(output) = tty.output.as_mut() else {
        return;
    };
    match output.surface.frame_submitted() {
        Ok(Some(())) => {}
        // No flip was pending: a stale event from before a pause.
        Ok(None) => return,
        Err(err) => return fatal(state, format!("page flip failed: {err}")),
    }
    render(state);
}

/// The session is going to another VT: give up the display and input.
fn pause(state: &mut State) {
    info!("session paused");
    let tty = state.backend.tty();
    tty.active = false;
    tty.libinput.suspend();
    tty.drm.pause();
    state.release_movement();
}

/// The session is back: restore the display and start drawing again.
fn resume(state: &mut State) {
    info!("session resumed");
    let tty = state.backend.tty();
    if let Err(err) = tty.libinput.resume() {
        // Without input we could not even quit; better to exit.
        return fatal(state, format!("could not resume input devices: {err:?}"));
    }
    // `true`: reset the display to a known state, so that the next frame sets
    // the mode again. Another VT may have changed anything.
    if let Err(err) = tty.drm.activate(true) {
        return fatal(state, format!("could not take the display back: {err}"));
    }
    tty.active = true;
    if let Some(output) = tty.output.as_mut() {
        // A flip that was in the air when we paused never completes; forget
        // it, or no new frame would be sent while we wait for it.
        let _ = output.surface.frame_submitted();
    }
    render(state);
}

/// Restart the frame cycle if it has stopped.
fn check_stall(state: &mut State) {
    let tty = state.backend.tty();
    let stalled = tty.active
        && tty
            .output
            .as_ref()
            .is_some_and(|output| output.last_queued.elapsed() > STALL_TIMEOUT);
    if stalled {
        warn!("no page flip completed; restarting the frame cycle");
        if let Some(output) = tty.output.as_mut() {
            let _ = output.surface.frame_submitted();
        }
        render(state);
    }
}

/// Draw a frame and queue it for the next vblank.
fn render(state: &mut State) {
    if let Err(err) = try_render(state) {
        // Losing the session between the check and the flip is not an error.
        if state.backend.tty().active {
            fatal(state, format!("rendering failed: {err}"));
        }
    }
}

fn try_render(state: &mut State) -> Result<(), Box<dyn Error>> {
    let tty = state.backend.tty();
    let Some(output) = tty.output.as_ref() else {
        return Ok(());
    };
    if !tty.active {
        return Ok(());
    }
    let params = FrameParams {
        size: output.size,
        scale: output.scale,
        // A dmabuf's first row is the top of the picture.
        transform: Transform::Normal,
        // There is no host to show a cursor.
        builtin_cursor: true,
    };
    let wl_output = output.output.clone();
    let start = tty.frame.begin();

    let prepared = frame::prepare(state, &start, params, |_| {})?;

    let tty = state.backend.tty();
    let output = tty.output.as_mut().ok_or("the output went away")?;
    let (mut dmabuf, _age) = output.surface.next_buffer()?;
    let sync = {
        let mut target = tty.renderer.bind(&mut dmabuf)?;
        tty.frame
            .present(&prepared, &mut tty.renderer, &mut target)?
    };
    output.surface.queue_buffer(Some(sync), None, ())?;
    output.last_queued = Instant::now();

    frame::finish(state, &wl_output)?;
    state.backend.tty().frame.record(&start);
    Ok(())
}
