//! Planned: Windows Graphics Capture. Native dependencies belong in target-specific Cargo tables.
use crate::{BackendKind, stub::StubBackend};
use lisnap_core::CaptureBackend;

pub(crate) fn create() -> Box<dyn CaptureBackend> {
    Box::new(StubBackend(BackendKind::Windows))
}
