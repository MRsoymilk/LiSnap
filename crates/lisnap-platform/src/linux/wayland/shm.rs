use lisnap_core::{CaptureError, PixelSize};
use wayland_client::protocol::wl_shm;

pub(super) fn convert_rgba(
    format: wl_shm::Format,
    size: PixelSize,
    stride: u32,
    native: &[u8],
    y_invert: bool,
) -> Result<Vec<u8>, CaptureError> {
    if size.width == 0 || size.height == 0 {
        return Err(CaptureError::InvalidFrame("Wayland SHM dimensions must be non-zero".into()));
    }
    if !matches!(format, wl_shm::Format::Argb8888 | wl_shm::Format::Xrgb8888) {
        return Err(CaptureError::Unsupported(format!("Wayland SHM format {format:?} is not supported")));
    }

    let row_bytes = (size.width as usize)
        .checked_mul(4)
        .ok_or_else(|| CaptureError::InvalidFrame("Wayland SHM row size overflow".into()))?;
    if (stride as usize) < row_bytes {
        return Err(CaptureError::InvalidFrame("Wayland SHM stride is shorter than one row".into()));
    }
    let needed = (stride as usize)
        .checked_mul(size.height as usize)
        .ok_or_else(|| CaptureError::InvalidFrame("Wayland SHM byte length overflow".into()))?;
    if native.len() < needed {
        return Err(CaptureError::InvalidFrame("Wayland SHM data is shorter than its declared stride".into()));
    }
    let rgba_len = row_bytes
        .checked_mul(size.height as usize)
        .ok_or_else(|| CaptureError::InvalidFrame("Wayland RGBA byte length overflow".into()))?;
    let mut rgba = vec![0_u8; rgba_len];

    for dst_y in 0..size.height as usize {
        let src_y = if y_invert { size.height as usize - 1 - dst_y } else { dst_y };
        let src = &native[src_y * stride as usize..src_y * stride as usize + row_bytes];
        let dst = &mut rgba[dst_y * row_bytes..(dst_y + 1) * row_bytes];
        for (source, target) in src.chunks_exact(4).zip(dst.chunks_exact_mut(4)) {
            let alpha = if format == wl_shm::Format::Argb8888 { source[3] } else { 255 };
            let (red, green, blue) = if alpha == 0 {
                (0, 0, 0)
            } else if alpha < 255 && format == wl_shm::Format::Argb8888 {
                (unpremultiply(source[2], alpha), unpremultiply(source[1], alpha), unpremultiply(source[0], alpha))
            } else {
                (source[2], source[1], source[0])
            };
            target.copy_from_slice(&[red, green, blue, alpha]);
        }
    }

    Ok(rgba)
}

fn unpremultiply(channel: u8, alpha: u8) -> u8 {
    ((u32::from(channel) * 255 + u32::from(alpha) / 2) / u32::from(alpha)).min(255) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_short_stride_and_unsupported_format() {
        let size = PixelSize { width: 2, height: 1 };
        assert!(matches!(
            convert_rgba(wl_shm::Format::Xrgb8888, size, 4, &[0; 8], false),
            Err(CaptureError::InvalidFrame(_))
        ));
        assert!(matches!(
            convert_rgba(wl_shm::Format::Rgb565, size, 8, &[0; 8], false),
            Err(CaptureError::Unsupported(_))
        ));
    }
}
