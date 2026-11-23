use lisnap_core::PixelSize;

use super::annotation::RectI;

pub(crate) const PIN_HEADER_HEIGHT: i32 = 28;
const CLOSE_SIZE: i32 = 28;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PinLayout {
    pub(crate) bounds: RectI,
    pub(crate) image: RectI,
    pub(crate) header_height: i32,
}

impl PinLayout {
    pub(crate) fn translated(mut self, x: i32, y: i32) -> Option<Self> {
        self.bounds.x = self.bounds.x.checked_add(x)?;
        self.bounds.y = self.bounds.y.checked_add(y)?;
        self.image.x = self.image.x.checked_add(x)?;
        self.image.y = self.image.y.checked_add(y)?;
        Some(self)
    }

    pub(crate) fn close_button(self) -> RectI {
        RectI {
            x: self.bounds.x + (self.bounds.width - CLOSE_SIZE).max(0),
            y: self.bounds.y,
            width: self.bounds.width.min(CLOSE_SIZE),
            height: self.header_height,
        }
    }

    fn move_to(&mut self, x: i32, y: i32) {
        self.image.x += x - self.bounds.x;
        self.image.y += y - self.bounds.y;
        self.bounds.x = x;
        self.bounds.y = y;
    }
}

pub(crate) fn intersect(a: RectI, b: RectI) -> Option<RectI> {
    let x = a.x.max(b.x);
    let y = a.y.max(b.y);
    let right = (i64::from(a.x) + i64::from(a.width)).min(i64::from(b.x) + i64::from(b.width));
    let bottom = (i64::from(a.y) + i64::from(a.height)).min(i64::from(b.y) + i64::from(b.height));
    (right > i64::from(x) && bottom > i64::from(y)).then(|| RectI {
        x,
        y,
        width: (right - i64::from(x)) as i32,
        height: (bottom - i64::from(y)) as i32,
    })
}

pub(crate) fn global_pointer(output: RectI, point: (f64, f64)) -> (f64, f64) {
    // An implicit pointer grab continues reporting coordinates relative to the
    // original output even after the pointer has crossed onto another output.
    (f64::from(output.x) + point.0, f64::from(output.y) + point.1)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PinSurfaceClip {
    pub(crate) destination: RectI,
    pub(crate) source: (f64, f64, f64, f64),
}

pub(crate) fn clip_surface(surface: RectI, output: RectI, pixels: PixelSize) -> Option<PinSurfaceClip> {
    if surface.width <= 0 || surface.height <= 0 || pixels.width == 0 || pixels.height == 0 {
        return None;
    }
    let visible = intersect(surface, output)?;
    // Viewporter uses 24.8 fixed-point source coordinates. Compute shared edges
    // once in integer arithmetic so neighboring views have no gaps, overlaps,
    // or rounding beyond the backing buffer on fractionally scaled displays.
    let edge = |offset: i64, pixels: u32, logical: i32| {
        (i128::from(offset) * i128::from(pixels) * 256 / i128::from(logical)) as i64
    };
    let dx = i64::from(visible.x) - i64::from(surface.x);
    let dy = i64::from(visible.y) - i64::from(surface.y);
    let x1 = edge(dx, pixels.width, surface.width);
    let y1 = edge(dy, pixels.height, surface.height);
    let x2 = edge(dx + i64::from(visible.width), pixels.width, surface.width);
    let y2 = edge(dy + i64::from(visible.height), pixels.height, surface.height);
    if x2 <= x1 || y2 <= y1 {
        return None;
    }
    Some(PinSurfaceClip {
        destination: RectI { x: visible.x - output.x, y: visible.y - output.y, ..visible },
        source: (x1 as f64 / 256.0, y1 as f64 / 256.0, (x2 - x1) as f64 / 256.0, (y2 - y1) as f64 / 256.0),
    })
}

fn desktop_bounds(outputs: &[RectI]) -> Option<RectI> {
    let x = outputs.iter().map(|output| output.x).min()?;
    let y = outputs.iter().map(|output| output.y).min()?;
    if outputs.iter().any(|output| output.width <= 0 || output.height <= 0) {
        return None;
    }
    let right = outputs.iter().map(|o| i64::from(o.x) + i64::from(o.width)).max()?;
    let bottom = outputs.iter().map(|o| i64::from(o.y) + i64::from(o.height)).max()?;
    i32::try_from(right).ok()?;
    i32::try_from(bottom).ok()?;
    Some(RectI {
        x,
        y,
        width: i32::try_from(right - i64::from(x)).ok()?,
        height: i32::try_from(bottom - i64::from(y)).ok()?,
    })
}

pub(crate) fn pin_layout(selection: RectI, screen: PixelSize) -> Option<PinLayout> {
    let width = i32::try_from(screen.width).ok()?;
    let height = i32::try_from(screen.height).ok()?;
    if width <= 0 || height < 2 || selection.width <= 0 || selection.height <= 0 {
        return None;
    }
    let header_height = PIN_HEADER_HEIGHT.min(height - 1);
    let scale = (f64::from(width) / f64::from(selection.width))
        .min(f64::from(height - header_height) / f64::from(selection.height))
        .min(1.0);
    let image_width = (f64::from(selection.width) * scale).floor().max(1.0) as i32;
    let image_height = (f64::from(selection.height) * scale).floor().max(1.0) as i32;
    let frame_width = image_width.max(CLOSE_SIZE.min(width));
    let frame_height = image_height + header_height;
    let x = selection.x.clamp(0, width - frame_width);
    let y = selection.y.clamp(0, height - frame_height);
    Some(PinLayout {
        bounds: RectI { x, y, width: frame_width, height: frame_height },
        image: RectI {
            x: x + (frame_width - image_width) / 2,
            y: y + header_height,
            width: image_width,
            height: image_height,
        },
        header_height,
    })
}

#[derive(Debug, Clone, Copy)]
struct PinDrag {
    pointer: (f64, f64),
    origin: (i32, i32),
}

#[derive(Debug)]
pub(crate) struct PinInteraction {
    pub(crate) layout: PinLayout,
    outputs: Vec<RectI>,
    desktop: RectI,
    pointer: Option<(f64, f64)>,
    drag: Option<PinDrag>,
}

impl PinInteraction {
    #[cfg(test)]
    pub(crate) fn new(layout: PinLayout, screen: PixelSize) -> Self {
        Self::with_outputs(layout, vec![RectI { x: 0, y: 0, width: screen.width as i32, height: screen.height as i32 }])
            .unwrap()
    }

    pub(crate) fn with_outputs(layout: PinLayout, outputs: Vec<RectI>) -> Option<Self> {
        let desktop = desktop_bounds(&outputs)?;
        Some(Self { layout, outputs, desktop, pointer: None, drag: None })
    }

    pub(crate) fn update_outputs(&mut self, outputs: Vec<RectI>) {
        if self.outputs == outputs {
            return;
        }
        if let Some(desktop) = desktop_bounds(&outputs) {
            self.outputs = outputs;
            self.desktop = desktop;
            self.release();
        }
    }

    pub(crate) fn motion(&mut self, point: (f64, f64)) -> bool {
        if !point.0.is_finite() || !point.1.is_finite() {
            return false;
        }
        self.pointer = Some(point);
        let Some(drag) = self.drag else { return false };
        // Coordinates come from the stationary, transparent parent, not the
        // moving image subsurface. This avoids feedback/jitter while dragging.
        let x = (f64::from(drag.origin.0) + point.0 - drag.pointer.0).round().clamp(
            f64::from(self.desktop.x),
            f64::from(self.desktop.x) + f64::from((self.desktop.width - self.layout.bounds.width).max(0)),
        ) as i32;
        let y = (f64::from(drag.origin.1) + point.1 - drag.pointer.1).round().clamp(
            f64::from(self.desktop.y),
            f64::from(self.desktop.y) + f64::from((self.desktop.height - self.layout.bounds.height).max(0)),
        ) as i32;
        if (x, y) == (self.layout.bounds.x, self.layout.bounds.y) {
            return false;
        }
        self.layout.move_to(x, y);
        true
    }

    /// Returns true when the close button was pressed.
    pub(crate) fn press(&mut self) -> bool {
        let Some(point) = self.pointer else { return false };
        if self.layout.close_button().contains(point) {
            return true;
        }
        if self.layout.bounds.contains(point) {
            self.drag = Some(PinDrag { pointer: point, origin: (self.layout.bounds.x, self.layout.bounds.y) });
        }
        false
    }

    pub(crate) fn release(&mut self) -> bool {
        self.drag = None;
        if self.outputs.iter().any(|output| intersect(self.layout.bounds, *output).is_some()) {
            return false;
        }
        // A layout gap or a disconnected monitor must not strand the entire pin
        // outside every visible output. Restore it to the nearest available one.
        let nearest = self
            .outputs
            .iter()
            .map(|output| {
                let x =
                    self.layout.bounds.x.clamp(output.x, output.x + (output.width - self.layout.bounds.width).max(0));
                let y =
                    self.layout.bounds.y.clamp(output.y, output.y + (output.height - self.layout.bounds.height).max(0));
                let dx = f64::from(x) - f64::from(self.layout.bounds.x);
                let dy = f64::from(y) - f64::from(self.layout.bounds.y);
                (dx * dx + dy * dy, x, y)
            })
            .min_by(|a, b| a.0.total_cmp(&b.0));
        if let Some((_, x, y)) = nearest {
            self.layout.move_to(x, y);
            return true;
        }
        false
    }

    pub(crate) fn leave(&mut self) {
        self.pointer = None;
    }

    pub(crate) fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    pub(crate) fn contains_pointer(&self) -> bool {
        self.pointer.is_some_and(|point| self.layout.bounds.contains(point))
    }

    pub(crate) fn over_close_button(&self) -> bool {
        self.pointer.is_some_and(|point| self.layout.close_button().contains(point))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: PixelSize = PixelSize { width: 1000, height: 600 };
    const SELECTION: RectI = RectI { x: 100, y: 80, width: 400, height: 200 };

    #[test]
    fn image_retains_logical_size_and_header_has_close_button() {
        let layout = pin_layout(SELECTION, SCREEN).unwrap();
        assert_eq!(layout.bounds, RectI { x: 100, y: 80, width: 400, height: 228 });
        assert_eq!(layout.image, RectI { x: 100, y: 108, width: 400, height: 200 });
        assert!(layout.close_button().contains((490.0, 90.0)));
        assert!(!layout.close_button().contains((490.0, 150.0)));
    }

    #[test]
    fn large_and_tiny_pins_fit_screen_without_distorting_or_losing_close_button() {
        for (selection, screen) in [
            (RectI { x: 0, y: 0, width: 2000, height: 1000 }, SCREEN),
            (RectI { x: 999, y: 599, width: 1, height: 1 }, SCREEN),
            (RectI { x: 0, y: 0, width: 1, height: 1 }, PixelSize { width: 10, height: 10 }),
        ] {
            let layout = pin_layout(selection, screen).unwrap();
            assert!(layout.bounds.x >= 0 && layout.bounds.y >= 0);
            assert!(layout.bounds.x + layout.bounds.width <= screen.width as i32);
            assert!(layout.bounds.y + layout.bounds.height <= screen.height as i32);
            assert!(layout.image.width > 0 && layout.image.height > 0 && layout.header_height > 0);
            assert!(layout.close_button().width > 0);
        }
        assert!(pin_layout(SELECTION, PixelSize { width: 0, height: 0 }).is_none());
    }

    #[test]
    fn drag_uses_absolute_pointer_offset_and_repeated_events_do_not_accumulate_drift() {
        let mut pin = PinInteraction::new(pin_layout(SELECTION, SCREEN).unwrap(), SCREEN);
        pin.motion((200.0, 150.0));
        assert!(!pin.press());
        assert!(pin.is_dragging());
        assert!(pin.motion((300.0, 200.0)));
        assert_eq!((pin.layout.bounds.x, pin.layout.bounds.y), (200, 130));
        assert!(!pin.motion((300.0, 200.0)));
        assert!(pin.motion((320.0, 210.0)));
        assert_eq!((pin.layout.bounds.x, pin.layout.bounds.y), (220, 140));
        assert_eq!((pin.layout.image.x, pin.layout.image.y), (220, 168));
        pin.release();
        assert!(!pin.motion((400.0, 400.0)));
        assert!(!pin.is_dragging());
    }

    #[test]
    fn drag_clamps_to_output_edges_and_close_button_is_not_a_drag_handle() {
        let mut pin = PinInteraction::new(pin_layout(SELECTION, SCREEN).unwrap(), SCREEN);
        pin.motion((490.0, 90.0));
        assert!(pin.press());
        assert!(!pin.is_dragging());
        pin.motion((200.0, 150.0));
        pin.press();
        pin.motion((-1000.0, -1000.0));
        assert_eq!((pin.layout.bounds.x, pin.layout.bounds.y), (0, 0));
        pin.motion((2000.0, 2000.0));
        assert_eq!((pin.layout.bounds.x, pin.layout.bounds.y), (600, 372));
        pin.release();
        pin.leave();
        assert!(!pin.press());
        assert!(!pin.contains_pointer());
    }

    #[test]
    fn drag_crosses_outputs_while_grab_coordinates_remain_relative_to_source() {
        let source = RectI { x: 0, y: 0, width: 1000, height: 600 };
        let target = RectI { x: 1000, y: 0, width: 800, height: 600 };
        let mut pin =
            PinInteraction::with_outputs(pin_layout(SELECTION, SCREEN).unwrap(), vec![source, target]).unwrap();
        pin.motion(global_pointer(source, (200.0, 150.0)));
        pin.press();
        pin.motion(global_pointer(source, (1350.0, 250.0)));
        assert_eq!((pin.layout.bounds.x, pin.layout.bounds.y), (1250, 180));
        assert!(intersect(pin.layout.bounds, source).is_none());
        assert_eq!(intersect(pin.layout.bounds, target), Some(pin.layout.bounds));
        pin.release();
        // After the implicit grab ends, events come from the destination view.
        pin.motion(global_pointer(target, (636.0, 194.0)));
        assert!(pin.over_close_button());
        assert!(pin.press());
    }

    #[test]
    fn drag_supports_left_and_upper_monitors_with_negative_logical_origins() {
        let primary = RectI { x: 0, y: 0, width: 1000, height: 600 };
        let left = RectI { x: -800, y: 100, width: 800, height: 600 };
        let upper = RectI { x: -200, y: -600, width: 1200, height: 600 };
        let layout = pin_layout(SELECTION, SCREEN).unwrap();
        let mut pin = PinInteraction::with_outputs(layout, vec![primary, left, upper]).unwrap();
        pin.motion((200.0, 150.0));
        pin.press();
        pin.motion(global_pointer(primary, (-550.0, 250.0)));
        assert_eq!((pin.layout.bounds.x, pin.layout.bounds.y), (-650, 180));
        assert_eq!(intersect(pin.layout.bounds, left), Some(pin.layout.bounds));
        assert_eq!(global_pointer(left, (150.0, 80.0)), (-650.0, 180.0));
        pin.motion((300.0, -350.0));
        assert_eq!((pin.layout.bounds.x, pin.layout.bounds.y), (200, -420));
        assert_eq!(intersect(pin.layout.bounds, upper), Some(pin.layout.bounds));
        assert_eq!(layout.translated(-800, -600).unwrap().bounds.x, -700);
    }

    #[test]
    fn per_output_clips_share_exact_pixel_edges_on_fractionally_scaled_images() {
        let image = RectI { x: 901, y: 100, width: 1017, height: 600 };
        let pixels = PixelSize { width: 1525, height: 900 };
        let left = RectI { x: 0, y: 0, width: 1000, height: 900 };
        let right = RectI { x: 1000, y: 0, width: 1000, height: 900 };
        let a = clip_surface(image, left, pixels).unwrap();
        let b = clip_surface(image, right, pixels).unwrap();
        assert_eq!(a.destination, RectI { x: 901, y: 100, width: 99, height: 600 });
        assert_eq!(b.destination, RectI { x: 0, y: 100, width: 918, height: 600 });
        assert_eq!(a.source.0 + a.source.2, b.source.0);
        assert_eq!(b.source.0 + b.source.2, 1525.0);
        assert_eq!(a.source.3, 900.0);
        assert_eq!(a.destination.width + b.destination.width, image.width);
        assert!(clip_surface(image, RectI { x: -1000, ..left }, pixels).is_none());
    }

    #[test]
    fn mixed_resolution_outputs_clip_in_logical_coordinates_not_physical_pixels() {
        // The source capture is 2x physical resolution; the second output may
        // use another scale. Only logical rectangles govern drag and clipping.
        let image = RectI { x: -200, y: -100, width: 800, height: 400 };
        let pixels = PixelSize { width: 1600, height: 800 };
        let upper = RectI { x: -200, y: -600, width: 1200, height: 600 };
        let lower = RectI { x: 0, y: 0, width: 1000, height: 600 };
        let a = clip_surface(image, upper, pixels).unwrap();
        let b = clip_surface(image, lower, pixels).unwrap();
        assert_eq!(a.source, (0.0, 0.0, 1600.0, 200.0));
        assert_eq!(b.source, (400.0, 200.0, 1200.0, 600.0));
        assert_eq!(b.destination, RectI { x: 0, y: 0, width: 600, height: 300 });
    }

    #[test]
    fn layout_gaps_and_output_removal_cannot_strand_the_pin_off_desktop() {
        let left = RectI { x: 0, y: 0, width: 600, height: 600 };
        let right = RectI { x: 1200, y: 0, width: 600, height: 600 };
        let layout = pin_layout(RectI { x: 100, y: 100, width: 200, height: 100 }, SCREEN).unwrap();
        let mut pin = PinInteraction::with_outputs(layout, vec![left, right]).unwrap();
        pin.motion((150.0, 150.0));
        pin.press();
        pin.motion((900.0, 150.0));
        assert!(intersect(pin.layout.bounds, left).is_none() && intersect(pin.layout.bounds, right).is_none());
        assert!(pin.release());
        assert_eq!(pin.layout.bounds.x, 1200);
        pin.update_outputs(vec![left]);
        assert_eq!(pin.layout.bounds.x, 400);
        assert!(!pin.is_dragging());
        assert!(intersect(pin.layout.bounds, left).is_some());
    }

    #[test]
    fn duplicate_output_configure_does_not_cancel_an_active_drag() {
        let output = RectI { x: 0, y: 0, width: 1000, height: 600 };
        let mut pin = PinInteraction::with_outputs(pin_layout(SELECTION, SCREEN).unwrap(), vec![output]).unwrap();
        pin.motion((200.0, 150.0));
        pin.press();
        pin.update_outputs(vec![output]);
        assert!(pin.is_dragging());
        pin.motion((400.0, 250.0));
        assert_eq!((pin.layout.bounds.x, pin.layout.bounds.y), (300, 180));
    }

    #[test]
    fn newly_connected_output_extends_the_available_drag_area() {
        let source = RectI { x: 0, y: 0, width: 1000, height: 600 };
        let target = RectI { x: 1000, y: 0, width: 800, height: 600 };
        let mut pin = PinInteraction::with_outputs(pin_layout(SELECTION, SCREEN).unwrap(), vec![source]).unwrap();
        pin.update_outputs(vec![source, target]);
        pin.motion((200.0, 150.0));
        pin.press();
        pin.motion((1350.0, 250.0));
        assert_eq!((pin.layout.bounds.x, pin.layout.bounds.y), (1250, 180));
        assert_eq!(intersect(pin.layout.bounds, target), Some(pin.layout.bounds));
    }
}
