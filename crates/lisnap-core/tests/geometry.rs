use lisnap_core::{LogicalRect, PixelRect, PixelSize, selection_to_pixels};

const OUTPUT: LogicalRect = LogicalRect { x: -1280.0, y: 0.0, width: 1280.0, height: 720.0 };
const PIXELS: PixelSize = PixelSize { width: 1920, height: 1080 };

#[test]
fn negative_output_origin_and_fractional_scale() {
    let rect = LogicalRect { x: -1180.0, y: 100.0, width: 400.0, height: 300.0 };
    assert_eq!(selection_to_pixels(rect, OUTPUT, PIXELS), Some(PixelRect { x: 150, y: 150, width: 600, height: 450 }));
}

#[test]
fn normalizes_reverse_drag() {
    let rect = LogicalRect { x: -780.0, y: 400.0, width: -400.0, height: -300.0 };
    assert_eq!(selection_to_pixels(rect, OUTPUT, PIXELS), Some(PixelRect { x: 150, y: 150, width: 600, height: 450 }));
}

#[test]
fn clips_to_one_output() {
    let rect = LogicalRect { x: -1300.0, y: -20.0, width: 1320.0, height: 800.0 };
    assert_eq!(selection_to_pixels(rect, OUTPUT, PIXELS), Some(PixelRect { x: 0, y: 0, width: 1920, height: 1080 }));
}

#[test]
fn rounds_edges_outwards() {
    let rect = LogicalRect { x: -1279.5, y: 0.5, width: 1.0, height: 1.0 };
    assert_eq!(selection_to_pixels(rect, OUTPUT, PIXELS), Some(PixelRect { x: 0, y: 0, width: 3, height: 3 }));
}

#[test]
fn ignores_empty_outside_and_nonfinite_selections() {
    for rect in [
        LogicalRect { x: 10.0, y: 0.0, width: 10.0, height: 10.0 },
        LogicalRect { width: 0.0, ..OUTPUT },
        LogicalRect { x: f64::NAN, ..OUTPUT },
        LogicalRect { height: f64::INFINITY, ..OUTPUT },
    ] {
        assert_eq!(selection_to_pixels(rect, OUTPUT, PIXELS), None);
    }
}

#[test]
fn rejects_invalid_output_geometry() {
    assert_eq!(selection_to_pixels(OUTPUT, LogicalRect { width: -1.0, ..OUTPUT }, PIXELS), None);
    assert_eq!(selection_to_pixels(OUTPUT, OUTPUT, PixelSize { width: 0, height: 1080 }), None);
}

#[test]
fn derives_axis_scales_from_actual_pixel_size() {
    let output = LogicalRect { x: 0.0, y: -100.0, width: 100.0, height: 100.0 };
    let rect = LogicalRect { x: 25.0, y: -75.0, width: 50.0, height: 50.0 };
    assert_eq!(
        selection_to_pixels(rect, output, PixelSize { width: 200, height: 300 }),
        Some(PixelRect { x: 50, y: 75, width: 100, height: 150 })
    );
}

#[test]
fn exact_fractionally_scaled_edges_do_not_gain_an_extra_pixel() {
    assert_eq!(
        selection_to_pixels(
            LogicalRect { x: 100.0, y: 80.0, width: 300.0, height: 200.0 },
            LogicalRect { x: 0.0, y: 0.0, width: 1000.0, height: 500.0 },
            PixelSize { width: 1500, height: 750 },
        ),
        Some(PixelRect { x: 150, y: 120, width: 450, height: 300 }),
    );
}
