use std::{fmt, time::SystemTime};

use crate::{CaptureError, LogicalRect, PixelSize, RgbaImage};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendStatus {
    Available,
    NotImplemented,
    Unsupported,
}

impl fmt::Display for BackendStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Available => "available",
            Self::NotImplemented => "not implemented",
            Self::Unsupported => "unsupported on this OS",
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct BackendDescriptor {
    pub id: &'static str,
    pub name: &'static str,
    pub technology: &'static str,
    pub status: BackendStatus,
}

#[derive(Debug, Clone)]
pub struct ScreenInfo {
    /// Stable identifier supplied by the native backend, never an egui viewport ID.
    pub id: String,
    pub name: String,
    /// None when the compositor does not expose a global output layout.
    pub logical_geometry: Option<LogicalRect>,
    /// Dimensions after the backend has normalized rotation and reflection.
    pub pixel_size: PixelSize,
    pub scale_factor: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct CaptureFrame {
    pub screen: ScreenInfo,
    pub image: RgbaImage,
    /// Timestamp for this output, not a promise of synchronized multi-output capture.
    pub captured_at: SystemTime,
}

#[derive(Debug, Clone, Default)]
pub struct CaptureRequest {
    /// None requests all outputs; Some uses the backend's stable output ID.
    pub screen_id: Option<String>,
    pub include_cursor: bool,
}

/// Native implementations own their connection/thread-affinity requirements.
/// Do not perform blocking capture inside the UI repaint callback.
pub trait CaptureBackend {
    fn descriptor(&self) -> BackendDescriptor;
    fn screens(&mut self) -> Result<Vec<ScreenInfo>, CaptureError>;
    /// A successful native capture must return at least one real, immutable frame.
    fn capture(&mut self, request: &CaptureRequest) -> Result<Vec<CaptureFrame>, CaptureError>;
}
