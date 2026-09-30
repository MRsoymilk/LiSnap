use std::sync::Arc;

use crate::{CaptureError, PixelRect, PixelSize};

/// Immutable, tightly packed, orientation-normalized RGBA8 pixels (straight alpha).
/// Backends must normalize native row stride and channel order before construction.
#[derive(Debug, Clone)]
pub struct RgbaImage {
    size: PixelSize,
    pixels: Arc<[u8]>,
}

impl RgbaImage {
    pub fn new(size: PixelSize, pixels: Vec<u8>) -> Result<Self, CaptureError> {
        let expected = (size.width as usize).checked_mul(size.height as usize).and_then(|n| n.checked_mul(4));
        if size.width == 0 || size.height == 0 || expected != Some(pixels.len()) {
            return Err(CaptureError::InvalidFrame("RGBA8 dimensions and byte length do not match".into()));
        }
        Ok(Self { size, pixels: pixels.into() })
    }

    pub fn size(&self) -> PixelSize {
        self.size
    }
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// Crop the original pixels, never a composited UI canvas or its dark mask.
    pub fn crop(&self, rect: PixelRect) -> Result<Self, CaptureError> {
        let right = rect.x.checked_add(rect.width).ok_or(CaptureError::InvalidSelection)?;
        let bottom = rect.y.checked_add(rect.height).ok_or(CaptureError::InvalidSelection)?;
        if rect.width == 0 || rect.height == 0 || right > self.size.width || bottom > self.size.height {
            return Err(CaptureError::InvalidSelection);
        }
        let row_bytes = rect.width as usize * 4;
        let mut result = Vec::with_capacity(row_bytes * rect.height as usize);
        for y in rect.y..bottom {
            let start = (y as usize * self.size.width as usize + rect.x as usize) * 4;
            result.extend_from_slice(&self.pixels[start..start + row_bytes]);
        }
        Self::new(PixelSize { width: rect.width, height: rect.height }, result)
    }
}
