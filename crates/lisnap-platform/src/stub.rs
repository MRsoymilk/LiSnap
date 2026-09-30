use lisnap_core::{BackendDescriptor, CaptureBackend, CaptureError, CaptureFrame, CaptureRequest, ScreenInfo};

use crate::BackendKind;

pub(crate) struct StubBackend(pub(crate) BackendKind);

impl CaptureBackend for StubBackend {
    fn descriptor(&self) -> BackendDescriptor {
        self.0.descriptor()
    }

    fn screens(&mut self) -> Result<Vec<ScreenInfo>, CaptureError> {
        Err(CaptureError::NotImplemented(self.descriptor().id))
    }

    fn capture(&mut self, _request: &CaptureRequest) -> Result<Vec<CaptureFrame>, CaptureError> {
        Err(CaptureError::NotImplemented(self.descriptor().id))
    }
}
