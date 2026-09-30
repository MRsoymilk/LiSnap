use lisnap_core::{CaptureError, PixelRect, PixelSize, RgbaImage};

#[test]
fn validates_rgba_byte_count() {
    assert!(RgbaImage::new(PixelSize { width: 2, height: 2 }, vec![0; 15]).is_err());
    assert!(RgbaImage::new(PixelSize { width: 0, height: 1 }, vec![]).is_err());
    assert!(RgbaImage::new(PixelSize { width: u32::MAX, height: u32::MAX }, vec![]).is_err());
}

#[test]
fn crops_rows_without_mutating_original() {
    let image = RgbaImage::new(PixelSize { width: 3, height: 2 }, (0..24).collect()).unwrap();
    let cropped = image.crop(PixelRect { x: 1, y: 0, width: 1, height: 2 }).unwrap();
    assert_eq!(cropped.pixels(), &[4, 5, 6, 7, 16, 17, 18, 19]);
    assert_eq!(image.pixels(), &(0..24).collect::<Vec<_>>());
}

#[test]
fn rejects_empty_outside_and_overflowing_crops() {
    let image = RgbaImage::new(PixelSize { width: 2, height: 2 }, vec![0; 16]).unwrap();
    for rect in [
        PixelRect { x: 0, y: 0, width: 0, height: 1 },
        PixelRect { x: 1, y: 0, width: 2, height: 1 },
        PixelRect { x: u32::MAX, y: 0, width: 1, height: 1 },
    ] {
        assert!(matches!(image.crop(rect), Err(CaptureError::InvalidSelection)));
    }
}

#[test]
fn clones_share_immutable_pixel_storage() {
    let image = RgbaImage::new(PixelSize { width: 1, height: 1 }, vec![1, 2, 3, 4]).unwrap();
    let clone = image.clone();
    assert_eq!(image.pixels().as_ptr(), clone.pixels().as_ptr());
}
