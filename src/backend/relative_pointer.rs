//! Relative pointer motion for the nested backend.
//!
//! Mouselook needs "the mouse moved this far", with no screen edge to stop at.
//! A Wayland host offers exactly that through the relative-pointer protocol,
//! and winit even listens to it, but Smithay's winit backend only forwards
//! absolute pointer positions. So we listen for ourselves: this opens a second
//! event queue on the connection winit already has to the host, and creates
//! our own relative-pointer object on it. winit carries on reading the socket;
//! we only pick our events out of the queue once a frame.
//!
//! Locking and hiding the pointer is still done through winit.

use std::error::Error;

use smithay::reexports::winit::{raw_window_handle::RawDisplayHandle, window::Window};
use wayland_client::{
    Connection, Dispatch, EventQueue, QueueHandle, WEnum,
    backend::Backend,
    delegate_noop,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{wl_pointer::WlPointer, wl_registry::WlRegistry, wl_seat},
};
use wayland_protocols::wp::relative_pointer::zv1::client::{
    zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1,
    zwp_relative_pointer_v1::{self, ZwpRelativePointerV1},
};

pub struct RelativePointer {
    queue: EventQueue<Motion>,
    motion: Motion,
    // Kept so the objects stay alive on the host's side.
    _seat: wl_seat::WlSeat,
    _pointer: WlPointer,
    _relative_pointer: ZwpRelativePointerV1,
}

#[derive(Default)]
struct Motion {
    seat_has_pointer: bool,
    dx: f64,
    dy: f64,
}

impl RelativePointer {
    /// Start listening for relative motion on the host connection behind
    /// `window`. Fails if the host is not Wayland, lacks the protocol, or has
    /// no pointer.
    ///
    /// # Safety
    /// The returned value must not outlive the winit event loop that owns
    /// `window`'s connection. In practice: never drop it.
    pub unsafe fn new(window: &dyn Window) -> Result<Self, Box<dyn Error>> {
        let display = window.rwh_06_display_handle().display_handle()?;
        let RawDisplayHandle::Wayland(display) = display.as_raw() else {
            return Err("the host session is not Wayland".into());
        };

        // "Foreign" tells wayland-client that someone else owns the
        // connection and reads from it.
        let backend = unsafe { Backend::from_foreign_display(display.display.as_ptr().cast()) };
        let connection = Connection::from_backend(backend);

        let (globals, mut queue) = registry_queue_init::<Motion>(&connection)?;
        let handle = queue.handle();
        let manager: ZwpRelativePointerManagerV1 = globals.bind(&handle, 1..=1, ())?;
        let seat: wl_seat::WlSeat = globals.bind(&handle, 1..=5, ())?;

        // The seat announces whether it has a pointer in reply to the bind.
        let mut motion = Motion::default();
        queue.roundtrip(&mut motion)?;
        if !motion.seat_has_pointer {
            return Err("the host seat has no pointer".into());
        }

        let pointer = seat.get_pointer(&handle, ());
        let relative_pointer = manager.get_relative_pointer(&pointer, &handle, ());
        connection.flush()?;

        Ok(Self {
            queue,
            motion,
            _seat: seat,
            _pointer: pointer,
            _relative_pointer: relative_pointer,
        })
    }

    /// The pointer movement since the last call, in logical pixels (x right,
    /// y down), with the host's pointer acceleration applied.
    pub fn take_motion(&mut self) -> (f64, f64) {
        // winit has already read our events off the socket along with its
        // own; this hands the ones waiting in our queue to `Motion`.
        if let Err(err) = self.queue.dispatch_pending(&mut self.motion) {
            tracing::warn!(%err, "failed to dispatch relative pointer events");
        }
        (
            std::mem::take(&mut self.motion.dx),
            std::mem::take(&mut self.motion.dy),
        )
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for Motion {
    fn event(
        state: &mut Self,
        _seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _data: &(),
        _connection: &Connection,
        _handle: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(capabilities),
        } = event
        {
            state.seat_has_pointer = capabilities.contains(wl_seat::Capability::Pointer);
        }
    }
}

impl Dispatch<ZwpRelativePointerV1, ()> for Motion {
    fn event(
        state: &mut Self,
        _relative_pointer: &ZwpRelativePointerV1,
        event: zwp_relative_pointer_v1::Event,
        _data: &(),
        _connection: &Connection,
        _handle: &QueueHandle<Self>,
    ) {
        if let zwp_relative_pointer_v1::Event::RelativeMotion { dx, dy, .. } = event {
            state.dx += dx;
            state.dy += dy;
        }
    }
}

impl Dispatch<WlRegistry, GlobalListContents> for Motion {
    fn event(
        _state: &mut Self,
        _registry: &WlRegistry,
        _event: <WlRegistry as wayland_client::Proxy>::Event,
        _data: &GlobalListContents,
        _connection: &Connection,
        _handle: &QueueHandle<Self>,
    ) {
    }
}

// Our pointer object receives the usual enter, motion and button events too.
// winit handles those through its own pointer.
delegate_noop!(Motion: ignore WlPointer);
delegate_noop!(Motion: ZwpRelativePointerManagerV1);
