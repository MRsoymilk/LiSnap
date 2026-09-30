#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LogicalRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl LogicalRect {
    fn finite(self) -> bool {
        [self.x, self.y, self.width, self.height, self.x + self.width, self.y + self.height]
            .into_iter()
            .all(f64::is_finite)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelSize {
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Clip a logical selection to one output and round its half-open pixel edges outwards.
/// Actual pixel dimensions, rather than a nominal DPI scale, are authoritative.
/// Returns None for invalid geometry, empty selections, or no intersection.
pub fn selection_to_pixels(selection: LogicalRect, output: LogicalRect, pixels: PixelSize) -> Option<PixelRect> {
    if !selection.finite()
        || !output.finite()
        || output.width <= 0.0
        || output.height <= 0.0
        || pixels.width == 0
        || pixels.height == 0
    {
        return None;
    }
    let left = selection.x.min(selection.x + selection.width).max(output.x);
    let top = selection.y.min(selection.y + selection.height).max(output.y);
    let right = selection.x.max(selection.x + selection.width).min(output.x + output.width);
    let bottom = selection.y.max(selection.y + selection.height).min(output.y + output.height);
    if left >= right || top >= bottom {
        return None;
    }
    let x = |edge: f64| (edge - output.x) / output.width * f64::from(pixels.width);
    let y = |edge: f64| (edge - output.y) / output.height * f64::from(pixels.height);
    let x0 = x(left).floor().clamp(0.0, f64::from(pixels.width)) as u32;
    let y0 = y(top).floor().clamp(0.0, f64::from(pixels.height)) as u32;
    let x1 = x(right).ceil().clamp(0.0, f64::from(pixels.width)) as u32;
    let y1 = y(bottom).ceil().clamp(0.0, f64::from(pixels.height)) as u32;
    (x1 > x0 && y1 > y0).then_some(PixelRect { x: x0, y: y0, width: x1 - x0, height: y1 - y0 })
}
