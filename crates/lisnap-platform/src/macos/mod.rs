//! Planned: ScreenCaptureKit with normal system screen-recording authorization.
use crate::{BackendKind, stub::StubBackend};
use lisnap_core::CaptureBackend;

pub(crate) fn create() -> Box<dyn CaptureBackend> {
    Box::new(StubBackend(BackendKind::MacOS))
}
