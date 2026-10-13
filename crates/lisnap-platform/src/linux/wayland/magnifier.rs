use lisnap_core::{PixelSize, RgbaImage};

use super::annotation::RectI;

pub(crate) const PIXEL_RADIUS: i32 = 5;
pub(crate) const PIXEL_GRID: usize = (PIXEL_RADIUS * 2 + 1) as usize;
pub(crate) const PIXEL_ZOOM: i32 = 12;
pub(crate) const MAGNIFIER_SIZE: PixelSize = PixelSize { width: 220, height: 238 };

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PixelSample {
    pub(crate) x: u32,
    pub(crate) y: u32,
    pub(crate) color: [u8; 4],
    pub(crate) neighbors: [[u8; 4]; PIXEL_GRID * PIXEL_GRID],
}

impl PixelSample {
    pub(crate) fn hex_color(&self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.color[0], self.color[1], self.color[2])
    }

    pub(crate) fn rgb_color(&self) -> String {
        format!("RGB {},{},{}", self.color[0], self.color[1], self.color[2])
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MagnifierPreview {
    pub(crate) bounds: RectI,
    pub(crate) sample: PixelSample,
}

pub(crate) fn sample_pixel(image: &RgbaImage, logical_size: PixelSize, point: (f64, f64)) -> Option<PixelSample> {
    if logical_size.width == 0 || logical_size.height == 0 || !point.0.is_finite() || !point.1.is_finite() {
        return None;
    }
    let size = image.size();
    let x = (point.0.clamp(0.0, f64::from(logical_size.width)) * f64::from(size.width) / f64::from(logical_size.width))
        .floor()
        .clamp(0.0, f64::from(size.width - 1)) as u32;
    let y = (point.1.clamp(0.0, f64::from(logical_size.height)) * f64::from(size.height)
        / f64::from(logical_size.height))
    .floor()
    .clamp(0.0, f64::from(size.height - 1)) as u32;
    let at = |x: u32, y: u32| {
        let offset = (y as usize * size.width as usize + x as usize) * 4;
        let p = &image.pixels()[offset..offset + 4];
        [p[0], p[1], p[2], p[3]]
    };
    let neighbors = std::array::from_fn(|index| {
        let dx = (index % PIXEL_GRID) as i64 - i64::from(PIXEL_RADIUS);
        let dy = (index / PIXEL_GRID) as i64 - i64::from(PIXEL_RADIUS);
        let px = (i64::from(x) + dx).clamp(0, i64::from(size.width - 1)) as u32;
        let py = (i64::from(y) + dy).clamp(0, i64::from(size.height - 1)) as u32;
        at(px, py)
    });
    Some(PixelSample { x, y, color: at(x, y), neighbors })
}

pub(crate) fn magnifier_layout(point: (f64, f64), screen: PixelSize) -> RectI {
    let screen_width = i32::try_from(screen.width).unwrap_or(i32::MAX);
    let screen_height = i32::try_from(screen.height).unwrap_or(i32::MAX);
    let width = (MAGNIFIER_SIZE.width as i32).min(screen_width);
    let height = (MAGNIFIER_SIZE.height as i32).min(screen_height);
    let px = point.0.clamp(0.0, f64::from(screen_width)) as i32;
    let py = point.1.clamp(0.0, f64::from(screen_height)) as i32;
    let gap = 20;
    let x = if screen_width - px >= width + gap { px + gap } else { px - width - gap };
    let y = if screen_height - py >= height + gap { py + gap } else { py - height - gap };
    RectI { x: x.clamp(0, screen_width - width), y: y.clamp(0, screen_height - height), width, height }
}

pub(crate) fn magnifier_preview(
    image: &RgbaImage,
    logical_size: PixelSize,
    point: (f64, f64),
) -> Option<MagnifierPreview> {
    let sample = sample_pixel(image, logical_size, point)?;
    Some(MagnifierPreview { bounds: magnifier_layout(point, logical_size), sample })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_image() -> RgbaImage {
        let size = PixelSize { width: 20, height: 30 };
        let pixels = (0..size.height)
            .flat_map(|y| (0..size.width).flat_map(move |x| [x as u8, y as u8, (x + y) as u8, 255]))
            .collect();
        RgbaImage::new(size, pixels).unwrap()
    }

    #[test]
    fn pointer_maps_to_actual_pixels_with_fractional_and_different_axis_scales() {
        let image = test_image();
        let sample = sample_pixel(&image, PixelSize { width: 16, height: 15 }, (8.75, 5.75)).unwrap();
        assert_eq!((sample.x, sample.y), (10, 11));
        assert_eq!(sample.color, [10, 11, 21, 255]);
        assert_eq!(sample.hex_color(), "#0A0B15");
        assert_eq!(sample.rgb_color(), "RGB 10,11,21");
    }

    #[test]
    fn grid_center_is_sampled_pixel_and_neighbors_are_unfiltered_source_pixels() {
        let image = test_image();
        let sample = sample_pixel(&image, image.size(), (10.5, 15.5)).unwrap();
        assert_eq!(sample.neighbors[PIXEL_GRID * PIXEL_GRID / 2], sample.color);
        assert_eq!(sample.neighbors[0], [5, 10, 15, 255]);
        assert_eq!(sample.neighbors[PIXEL_GRID * PIXEL_GRID - 1], [15, 20, 35, 255]);
    }

    #[test]
    fn screen_edges_and_outside_drag_clamp_without_out_of_bounds_reads() {
        let image = test_image();
        let sample = sample_pixel(&image, image.size(), (-100.0, -100.0)).unwrap();
        assert_eq!(sample.color, [0, 0, 0, 255]);
        assert_eq!(sample.neighbors[0], sample.color);
        let sample = sample_pixel(&image, image.size(), (100.0, 100.0)).unwrap();
        assert_eq!((sample.x, sample.y), (19, 29));
        assert_eq!(sample.neighbors[PIXEL_GRID * PIXEL_GRID - 1], sample.color);
        let one = RgbaImage::new(PixelSize { width: 1, height: 1 }, vec![255, 0, 128, 255]).unwrap();
        let sample = sample_pixel(&one, one.size(), (0.0, 0.0)).unwrap();
        assert!(sample.neighbors.iter().all(|color| *color == [255, 0, 128, 255]));
        assert_eq!(sample.hex_color(), "#FF0080");
    }

    #[test]
    fn invalid_geometry_does_not_produce_a_sample() {
        let image = test_image();
        assert!(sample_pixel(&image, PixelSize { width: 0, height: 15 }, (0.0, 0.0)).is_none());
        assert!(sample_pixel(&image, image.size(), (f64::NAN, 0.0)).is_none());
        assert!(sample_pixel(&image, image.size(), (0.0, f64::INFINITY)).is_none());
    }

    #[test]
    fn popup_flips_at_screen_edges_and_stays_on_tiny_screens() {
        let screen = PixelSize { width: 1000, height: 600 };
        let point = (400.0, 200.0);
        let rect = magnifier_layout(point, screen);
        assert_eq!((rect.x, rect.y), (420, 220));
        assert!(!rect.contains(point));
        let rect = magnifier_layout((999.0, 599.0), screen);
        assert_eq!((rect.x, rect.y), (759, 341));
        assert!(!rect.contains((999.0, 599.0)));
        for screen in [screen, PixelSize { width: 10, height: 10 }] {
            for point in [(0.0, 0.0), (999.0, 599.0), (-100.0, -100.0)] {
                let rect = magnifier_layout(point, screen);
                assert!(rect.x >= 0 && rect.y >= 0);
                assert!(rect.x + rect.width <= screen.width as i32);
                assert!(rect.y + rect.height <= screen.height as i32);
            }
        }
    }
}
