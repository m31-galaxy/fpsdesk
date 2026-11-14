//! linux-dmabuf: lets clients that render on the GPU hand us their buffers
//! without a copy. Without it, GL and Vulkan clients fall back to software
//! rendering or fail to start.

use smithay::{
    backend::{
        allocator::dmabuf::Dmabuf,
        egl::EGLDevice,
        renderer::{ImportDma, gles::GlesRenderer},
    },
    reexports::wayland_server::DisplayHandle,
    wayland::dmabuf::{
        DmabufFeedbackBuilder, DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier,
    },
};
use tracing::warn;

use crate::state::State;

/// Advertise the dmabuf formats `renderer` can import.
pub fn init(dh: &DisplayHandle, renderer: &mut GlesRenderer) -> (DmabufState, DmabufGlobal) {
    let formats = renderer.dmabuf_formats();
    let mut state = DmabufState::new();

    // Protocol version 4 tells clients which GPU to render on, and Mesa's EGL
    // needs that. It requires knowing our render node.
    let render_node = EGLDevice::device_for_display(renderer.egl_context().display())
        .and_then(|device| device.try_get_render_node());
    let feedback = match render_node {
        Ok(Some(node)) => DmabufFeedbackBuilder::new(node.dev_id(), formats.clone())
            .build()
            .map_err(|err| warn!(%err, "failed to build dmabuf feedback"))
            .ok(),
        Ok(None) => {
            warn!("EGL device has no render node");
            None
        }
        Err(err) => {
            warn!(%err, "failed to query EGL device");
            None
        }
    };

    let global = match feedback {
        Some(feedback) => state.create_global_with_default_feedback::<State>(dh, &feedback),
        None => {
            warn!("falling back to dmabuf v3; GPU clients may not work");
            state.create_global::<State>(dh, formats)
        }
    };
    (state, global)
}

impl DmabufHandler for State {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.dmabuf_state
    }

    fn dmabuf_imported(
        &mut self,
        _global: &DmabufGlobal,
        dmabuf: Dmabuf,
        notifier: ImportNotifier,
    ) {
        // Importing here validates the buffer up front, so a client learns
        // immediately if we cannot display it.
        if self.backend.renderer().import_dmabuf(&dmabuf, None).is_ok() {
            let _ = notifier.successful::<State>();
        } else {
            notifier.failed();
        }
    }
}
