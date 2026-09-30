//! Planned: registry capability negotiation, direct capture into SHM; no Portal fallback.
use crate::{BackendKind, stub::StubBackend};
use lisnap_core::CaptureBackend;

pub(crate) fn create() -> Box<dyn CaptureBackend> {
    Box::new(StubBackend(BackendKind::Wayland))
}
