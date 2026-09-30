//! Planned: native X11 SHM with GetImage fallback. Not a Wayland desktop fallback.
use crate::{BackendKind, stub::StubBackend};
use lisnap_core::CaptureBackend;

pub(crate) fn create() -> Box<dyn CaptureBackend> {
    Box::new(StubBackend(BackendKind::X11))
}
