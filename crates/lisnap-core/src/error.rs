use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureError {
    NotImplemented(&'static str),
    Unsupported(String),
    PermissionDenied(String),
    InvalidFrame(String),
    InvalidSelection,
    Cancelled,
    Timeout,
    BackendFailure(String),
}

impl fmt::Display for CaptureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotImplemented(id) => write!(f, "{id}: native capture is not implemented yet"),
            Self::Unsupported(reason) => write!(f, "unsupported: {reason}"),
            Self::PermissionDenied(reason) => write!(f, "permission denied: {reason}"),
            Self::InvalidFrame(reason) => write!(f, "invalid frame: {reason}"),
            Self::InvalidSelection => f.write_str("selection is empty or outside the image"),
            Self::Cancelled => f.write_str("capture cancelled"),
            Self::Timeout => f.write_str("capture timed out"),
            Self::BackendFailure(reason) => write!(f, "capture failed: {reason}"),
        }
    }
}

impl std::error::Error for CaptureError {}
