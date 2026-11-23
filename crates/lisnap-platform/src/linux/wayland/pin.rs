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
    screen: PixelSize,
    pointer: Option<(f64, f64)>,
    drag: Option<PinDrag>,
}

impl PinInteraction {
    pub(crate) fn new(layout: PinLayout, screen: PixelSize) -> Self {
        Self { layout, screen, pointer: None, drag: None }
    }

    pub(crate) fn motion(&mut self, point: (f64, f64)) -> bool {
        if !point.0.is_finite() || !point.1.is_finite() {
            return false;
        }
        self.pointer = Some(point);
        let Some(drag) = self.drag else { return false };
        // Coordinates come from the stationary, transparent parent, not the
        // moving image subsurface. This avoids feedback/jitter while dragging.
        let x = (f64::from(drag.origin.0) + point.0 - drag.pointer.0)
            .round()
            .clamp(0.0, f64::from(self.screen.width) - f64::from(self.layout.bounds.width)) as i32;
        let y = (f64::from(drag.origin.1) + point.1 - drag.pointer.1)
            .round()
            .clamp(0.0, f64::from(self.screen.height) - f64::from(self.layout.bounds.height)) as i32;
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

    pub(crate) fn release(&mut self) {
        self.drag = None;
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
}
