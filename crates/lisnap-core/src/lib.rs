//! Platform- and UI-independent screenshot model. No window system or GPU dependencies.
mod capture;
mod error;
mod geometry;
mod image;
mod settings;

pub use capture::{
    BackendDescriptor, BackendStatus, CaptureBackend, CaptureCancellation, CaptureFrame, CaptureRequest, ScreenInfo,
};
pub use error::CaptureError;
pub use geometry::{LogicalRect, PixelRect, PixelSize, selection_to_pixels};
pub use image::RgbaImage;
pub use settings::AppSettings;
