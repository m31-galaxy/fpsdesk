//! TTY backend: FPSDesk as a real session on a virtual console.
//!
//! The simplest path that works: one GPU (the primary one), libseat for the
//! session, DRM/KMS with GBM buffers for the displays, EGL on GBM for
//! rendering, and libinput for input.
//!
//! One display is the primary (the internal panel if there is one, else the
//! first connected connector, at its preferred mode). The world is drawn for
//! it alone, and it is the only output Wayland clients see. Every other
//! connected display is a mirror: after the primary's frame is drawn, its
//! scanout buffer is imported as a texture and drawn, scaled to fit with the
//! aspect ratio kept, into the mirror's own buffer. Each mirror has its own
//! mode, CRTC and vblank; it gets the latest frame whenever its previous
//! flip has completed and is skipped otherwise, so it can never stall the
//! primary. A mirror that fails is dropped with a warning.
//!
//! Frames are driven by the primary's vblank: the first frame is rendered at
//! startup, and each page flip's completion triggers the next. When the
//! session is paused (switched to another VT) DRM and libinput are paused
//! with it and nothing is drawn; on resume the modes are restored and the
//! cycle restarted.
//!
//! Hot-plug: udev change events on the DRM device rescan the connectors, which
//! adds mirrors for newly connected displays and drops those that have gone.
//! If the primary itself is unplugged, a warning is logged and nothing else
//! changes: promoting a mirror would mean resizing the view and the Wayland
//! output under running clients. (If the primary's flips then fail, that is
//! fatal as for any primary error.)
//!
//! The safety bindings (VT switching and quitting) are in `input/emergency.rs`.
//! Any error on the primary while rendering or flipping is logged and stops
//! the event loop, so that FPSDesk exits and gives the console back instead of hanging.

use std::{
    error::Error,
    path::PathBuf,
    time::{Duration, Instant},
};

use smithay::{
    backend::{
        allocator::{
            Fourcc,
            dmabuf::Dmabuf,
            gbm::{GbmAllocator, GbmBufferFlags, GbmDevice},
        },
        drm::{DrmDevice, DrmDeviceFd, DrmDeviceNotifier, DrmEvent, GbmBufferedSurface},
        egl::{EGLContext, EGLDisplay},
        input::InputEvent,
        libinput::{LibinputInputBackend, LibinputSessionInterface},
        renderer::{
            Bind, ImportDma,
            gles::{GlesRenderer, GlesTexture},
        },
        session::{
            Event as SessionEvent, Session,
            libseat::{LibSeatSession, LibSeatSessionNotifier},
        },
        udev::{UdevBackend, UdevEvent, all_gpus, primary_gpu},
    },
    output::{Mode as OutputMode, Output, PhysicalProperties, Scale, Subpixel},
    reexports::{
        calloop::{
            EventLoop,
            timer::{TimeoutAction, Timer},
        },
        drm::control::{
            Device as ControlDevice, Mode as DrmMode, ModeTypeFlags, ResourceHandles, connector,
            crtc,
        },
        input::{Device as InputDevice, Libinput},
        rustix::fs::OFlags,
    },
    utils::{DeviceFd, Physical, Size, Transform},
};
use tracing::{debug, error, info, warn};

use super::Backend;
use crate::{
    frame::{self, Frame, FrameParams},
    render::MirrorPresenter,
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
    connector: connector::Handle,
    crtc: crtc::Handle,
    size: Size<i32, Physical>,
    scale: f64,
    /// When a frame last went to the display, for spotting stalls.
    last_queued: Instant,
}

/// A display that shows the primary's picture.
struct Mirror {
    name: String,
    connector: connector::Handle,
    surface: Surface,
    crtc: crtc::Handle,
    size: Size<i32, Physical>,
    presenter: MirrorPresenter,
    /// Whether a page flip is in the air. No frame is sent meanwhile.
    pending: bool,
    last_queued: Instant,
}

/// Everything the TTY backend owns. Fields drop in order, so the display and
/// GPU go before the session that lent them.
pub struct Tty {
    output: Option<TtyOutput>,
    mirrors: Vec<Mirror>,
    /// Whether we have said that the primary's frame cannot be used as a texture.
    import_warned: bool,
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
    udev: UdevBackend,
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
            mirrors: Vec::new(),
            import_warned: false,
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
            udev: UdevBackend::new(&seat)?,
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
        self.mirrors.clear();
        self.output = None;
    }

    /// Choose the primary display and set up the output on it, then add a
    /// mirror for each other connected display.
    fn create_output(
        &mut self,
        dh: &smithay::reexports::wayland_server::DisplayHandle,
        scale: f64,
    ) -> Result<Output, Box<dyn Error>> {
        let resources = self.drm.resource_handles()?;
        let mut chosen = None;
        for info in self.connected(&resources, true) {
            if info.modes().is_empty() {
                warn!(output = connector_name(&info), "connected but has no modes");
                continue;
            }
            match self.crtc_for(&info, &resources, &[]) {
                Some(crtc) => {
                    chosen = Some((info, crtc));
                    break;
                }
                None => warn!(output = connector_name(&info), "no CRTC can drive it"),
            }
        }
        let (info, crtc) = chosen.ok_or("no connected display found")?;
        let name = connector_name(&info);
        let (surface, drm_mode) = self.open_surface(&info, crtc)?;
        info!(
            output = name,
            mode = ?drm_mode.size(),
            refresh = drm_mode.vrefresh(),
            ?crtc,
            "driving primary output"
        );

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
            connector: info.handle(),
            crtc,
            size: mode.size,
            scale,
            last_queued: Instant::now(),
        });
        self.sync_mirrors(true);
        Ok(output)
    }

    /// A scanout surface on `crtc` for `info`, at the connector's preferred
    /// mode.
    fn open_surface(
        &mut self,
        info: &connector::Info,
        crtc: crtc::Handle,
    ) -> Result<(Surface, DrmMode), Box<dyn Error>> {
        let drm_mode = info
            .modes()
            .iter()
            .find(|mode| mode.mode_type().contains(ModeTypeFlags::PREFERRED))
            .or_else(|| info.modes().first())
            .copied()
            .ok_or("the connector has no modes")?;
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
        Ok((surface, drm_mode))
    }

    /// The connected connectors, the internal panel first. `probe` asks the
    /// kernel to detect again rather than use what it last found.
    fn connected(&self, resources: &ResourceHandles, probe: bool) -> Vec<connector::Info> {
        let mut connected: Vec<connector::Info> = resources
            .connectors()
            .iter()
            .filter_map(|handle| self.drm.get_connector(*handle, probe).ok())
            .filter(|info| info.state() == connector::State::Connected)
            .collect();
        // Stable, so otherwise the kernel's order stands.
        connected.sort_by_key(|info| !is_internal(info));
        connected
    }

    /// Bring the mirrors in line with the connected displays: drop those
    /// that have gone, add one for each new display.
    fn sync_mirrors(&mut self, probe: bool) {
        let resources = match self.drm.resource_handles() {
            Ok(resources) => resources,
            Err(err) => return warn!(%err, "could not list the connectors"),
        };
        let connected = self.connected(&resources, probe);
        let is_connected = |handle| connected.iter().any(|info| info.handle() == handle);

        self.mirrors.retain(|mirror| {
            let keep = is_connected(mirror.connector);
            if !keep {
                info!(output = mirror.name, "display unplugged; mirror removed");
            }
            keep
        });
        if let Some(primary) = &self.output
            && !is_connected(primary.connector)
        {
            warn!(
                output = primary.output.name(),
                "the primary display is no longer connected; carrying on"
            );
        }

        for info in &connected {
            let handle = info.handle();
            let primary = self.output.as_ref().map(|output| output.connector);
            if primary == Some(handle) || self.mirrors.iter().any(|m| m.connector == handle) {
                continue;
            }
            let name = connector_name(info);
            if info.modes().is_empty() {
                warn!(output = name, "connected but has no modes");
                continue;
            }
            let taken: Vec<crtc::Handle> = self
                .output
                .iter()
                .map(|output| output.crtc)
                .chain(self.mirrors.iter().map(|mirror| mirror.crtc))
                .collect();
            let Some(crtc) = self.crtc_for(info, &resources, &taken) else {
                warn!(output = name, "no free CRTC can drive it; not mirrored");
                continue;
            };
            match self.open_surface(info, crtc) {
                Ok((surface, drm_mode)) => {
                    info!(
                        output = name,
                        mode = ?drm_mode.size(),
                        refresh = drm_mode.vrefresh(),
                        ?crtc,
                        "mirroring the primary"
                    );
                    self.mirrors.push(Mirror {
                        name,
                        connector: handle,
                        surface,
                        crtc,
                        size: OutputMode::from(drm_mode).size,
                        presenter: MirrorPresenter::new(),
                        pending: false,
                        last_queued: Instant::now(),
                    });
                }
                Err(err) => warn!(output = name, %err, "could not mirror the display"),
            }
        }
    }

    /// Show the primary's finished frame, in `dmabuf`, on each mirror that is
    /// ready for one. A mirror that errors is dropped.
    fn draw_mirrors(&mut self, dmabuf: &Dmabuf) {
        if self.mirrors.iter().all(|mirror| mirror.pending) {
            return;
        }
        // The same GPU memory the primary was just drawn into; commands on
        // this context run in order, so it is complete by the time it is read.
        let texture = match self.renderer.import_dmabuf(dmabuf, None) {
            Ok(texture) => texture,
            Err(err) => {
                if !self.import_warned {
                    warn!(%err, "could not read the primary's frame as a texture; mirrors stay blank");
                    self.import_warned = true;
                }
                return;
            }
        };
        let renderer = &mut self.renderer;
        self.mirrors.retain_mut(|mirror| {
            if mirror.pending {
                return true;
            }
            match draw_mirror(renderer, mirror, &texture) {
                Ok(()) => {
                    mirror.pending = true;
                    mirror.last_queued = Instant::now();
                    true
                }
                Err(err) => {
                    warn!(output = mirror.name, %err, "mirror failed; dropping it");
                    false
                }
            }
        });
    }

    /// A CRTC that can drive `info` and is not in `taken`: the one it is
    /// already on if it is on one (to avoid a needless modeset), else any
    /// that its encoders allow.
    fn crtc_for(
        &self,
        info: &connector::Info,
        resources: &ResourceHandles,
        taken: &[crtc::Handle],
    ) -> Option<crtc::Handle> {
        let encoders: Vec<_> = info
            .current_encoder()
            .into_iter()
            .chain(info.encoders().iter().copied())
            .filter_map(|handle| self.drm.get_encoder(handle).ok())
            .collect();
        encoders
            .iter()
            .filter_map(|encoder| encoder.crtc())
            .find(|crtc| !taken.contains(crtc))
            .or_else(|| {
                encoders.iter().find_map(|encoder| {
                    resources
                        .filter_crtcs(encoder.possible_crtcs())
                        .into_iter()
                        .find(|crtc| !taken.contains(crtc))
                })
            })
    }
}

/// Draw the primary's picture into the mirror's next buffer and queue it.
fn draw_mirror(
    renderer: &mut GlesRenderer,
    mirror: &mut Mirror,
    texture: &GlesTexture,
) -> Result<(), Box<dyn Error>> {
    let (mut dmabuf, _age) = mirror.surface.next_buffer()?;
    let sync = {
        let mut target = renderer.bind(&mut dmabuf)?;
        mirror
            .presenter
            .present(renderer, &mut target, mirror.size, texture.clone())?
    };
    mirror.surface.queue_buffer(Some(sync), None, ())?;
    Ok(())
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
        DrmEvent::VBlank(crtc) => on_vblank(state, crtc),
        DrmEvent::Error(err) => {
            // While switched away, errors are expected noise.
            if state.backend.tty().active {
                fatal(state, format!("DRM error: {err}"));
            } else {
                debug!(%err, "DRM error while paused");
            }
        }
    })?;

    // A display being plugged in or out changes the DRM device.
    handle.insert_source(events.udev, |event, _, state| {
        if let UdevEvent::Changed { device_id } = event {
            on_device_changed(state, device_id);
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

/// The DRM device changed, which is how a display being plugged in or out
/// shows up: look at the connectors again.
fn on_device_changed(state: &mut State, device_id: u64) {
    let tty = state.backend.tty();
    if device_id != tty.drm.device_id() {
        return;
    }
    // While paused, we do not own the display; resuming looks again.
    if tty.active {
        debug!("display configuration changed");
        tty.sync_mirrors(false);
    }
}

/// A page flip on `crtc` completed. For the primary, draw the next frame;
/// for a mirror, it can take another.
fn on_vblank(state: &mut State, crtc: crtc::Handle) {
    let tty = state.backend.tty();
    if let Some(index) = tty.mirrors.iter().position(|mirror| mirror.crtc == crtc) {
        let mirror = &mut tty.mirrors[index];
        match mirror.surface.frame_submitted() {
            Ok(_) => mirror.pending = false,
            Err(err) => {
                warn!(output = mirror.name, %err, "mirror page flip failed; dropping it");
                tty.mirrors.remove(index);
            }
        }
        return;
    }
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
    for mirror in &mut tty.mirrors {
        let _ = mirror.surface.frame_submitted();
        mirror.pending = false;
    }
    // Displays may have come or gone while we were away.
    tty.sync_mirrors(true);
    render(state);
}

/// Restart the frame cycle if it has stopped.
fn check_stall(state: &mut State) {
    let tty = state.backend.tty();
    if tty.active {
        // A mirror whose flip never completes (display switched off, say) is
        // let try again; if that fails too it is dropped.
        tty.mirrors.retain_mut(|mirror| {
            if !mirror.pending || mirror.last_queued.elapsed() <= STALL_TIMEOUT {
                return true;
            }
            warn!(
                output = mirror.name,
                "mirror page flip did not complete; retrying"
            );
            mirror.pending = false;
            mirror.surface.frame_submitted().is_ok()
        });
    }
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
    // The primary is on its way; now the mirrors can have a copy.
    tty.draw_mirrors(&dmabuf);

    frame::finish(state, &wl_output)?;
    state.backend.tty().frame.record(&start);
    Ok(())
}
