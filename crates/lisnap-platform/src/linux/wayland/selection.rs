use lisnap_core::PixelSize;

use super::annotation::RectI;

const HANDLE_SIZE: i32 = 8;
const HIT_RADIUS: f64 = 7.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResizeHandle {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    Top,
    Bottom,
    Left,
    Right,
}

impl ResizeHandle {
    fn edges(self) -> (bool, bool, bool, bool) {
        use ResizeHandle::*;
        (
            matches!(self, TopLeft | BottomLeft | Left),
            matches!(self, TopRight | BottomRight | Right),
            matches!(self, TopLeft | TopRight | Top),
            matches!(self, BottomLeft | BottomRight | Bottom),
        )
    }
}

fn handle_centers(selection: RectI) -> [(ResizeHandle, (i32, i32)); 8] {
    let RectI { x, y, width, height } = selection;
    let right = x + width;
    let bottom = y + height;
    use ResizeHandle::*;
    [
        (TopLeft, (x, y)),
        (TopRight, (right, y)),
        (BottomLeft, (x, bottom)),
        (BottomRight, (right, bottom)),
        (Top, (x + width / 2, y)),
        (Bottom, (x + width / 2, bottom)),
        (Left, (x, y + height / 2)),
        (Right, (right, y + height / 2)),
    ]
}

pub(crate) fn resize_handle_rects(selection: RectI, screen: PixelSize) -> [RectI; 8] {
    let screen_width = i32::try_from(screen.width).unwrap_or(i32::MAX);
    let screen_height = i32::try_from(screen.height).unwrap_or(i32::MAX);
    let width = HANDLE_SIZE.min(screen_width);
    let height = HANDLE_SIZE.min(screen_height);
    handle_centers(selection).map(|(_, (x, y))| RectI {
        x: (x - width / 2).clamp(0, screen_width - width),
        y: (y - height / 2).clamp(0, screen_height - height),
        width,
        height,
    })
}

pub(crate) fn resize_hit_test(selection: RectI, point: (f64, f64)) -> Option<ResizeHandle> {
    // Corners take priority over edges and midpoints, including on tiny selections.
    if let Some((handle, _)) = handle_centers(selection).into_iter().find(|(_, (x, y))| {
        (point.0 - f64::from(*x)).abs() <= HIT_RADIUS && (point.1 - f64::from(*y)).abs() <= HIT_RADIUS
    }) {
        return Some(handle);
    }
    let left = f64::from(selection.x);
    let top = f64::from(selection.y);
    let right = f64::from(selection.x + selection.width);
    let bottom = f64::from(selection.y + selection.height);
    if (left..=right).contains(&point.0) {
        if (point.1 - top).abs() <= HIT_RADIUS {
            return Some(ResizeHandle::Top);
        }
        if (point.1 - bottom).abs() <= HIT_RADIUS {
            return Some(ResizeHandle::Bottom);
        }
    }
    if (top..=bottom).contains(&point.1) {
        if (point.0 - left).abs() <= HIT_RADIUS {
            return Some(ResizeHandle::Left);
        }
        if (point.0 - right).abs() <= HIT_RADIUS {
            return Some(ResizeHandle::Right);
        }
    }
    None
}

pub(crate) fn resized_selection(
    before: RectI,
    handle: ResizeHandle,
    anchor: (f64, f64),
    point: (f64, f64),
    screen: PixelSize,
) -> RectI {
    let (left, right, top, bottom) = handle.edges();
    let dx = point.0 - anchor.0;
    let dy = point.1 - anchor.1;
    let mut x1 = before.x;
    let mut y1 = before.y;
    let mut x2 = before.x + before.width;
    let mut y2 = before.y + before.height;
    if left {
        x1 = (f64::from(x1) + dx).round().clamp(0.0, f64::from(x2 - 1)) as i32;
    }
    if right {
        x2 = (f64::from(x2) + dx).round().clamp(f64::from(x1 + 1), f64::from(screen.width)) as i32;
    }
    if top {
        y1 = (f64::from(y1) + dy).round().clamp(0.0, f64::from(y2 - 1)) as i32;
    }
    if bottom {
        y2 = (f64::from(y2) + dy).round().clamp(f64::from(y1 + 1), f64::from(screen.height)) as i32;
    }
    RectI { x: x1, y: y1, width: x2 - x1, height: y2 - y1 }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SELECTION: RectI = RectI { x: 100, y: 80, width: 400, height: 200 };
    const SCREEN: PixelSize = PixelSize { width: 1000, height: 500 };

    #[test]
    fn corners_midpoints_and_entire_edges_are_interactive_but_interior_is_not() {
        for (handle, (x, y)) in handle_centers(SELECTION) {
            assert_eq!(resize_hit_test(SELECTION, (f64::from(x), f64::from(y))), Some(handle));
        }
        assert_eq!(resize_hit_test(SELECTION, (170.0, 76.0)), Some(ResizeHandle::Top));
        assert_eq!(resize_hit_test(SELECTION, (504.0, 170.0)), Some(ResizeHandle::Right));
        assert_eq!(resize_hit_test(SELECTION, (300.0, 180.0)), None);
        assert_eq!(resize_hit_test(SELECTION, (50.0, 50.0)), None);
    }

    #[test]
    fn each_handle_moves_only_its_edges_and_preserves_pointer_offset() {
        use ResizeHandle::*;
        for (handle, expected) in [
            (TopLeft, RectI { x: 120, y: 90, width: 380, height: 190 }),
            (TopRight, RectI { x: 100, y: 90, width: 420, height: 190 }),
            (BottomLeft, RectI { x: 120, y: 80, width: 380, height: 210 }),
            (BottomRight, RectI { x: 100, y: 80, width: 420, height: 210 }),
            (Top, RectI { x: 100, y: 90, width: 400, height: 190 }),
            (Bottom, RectI { x: 100, y: 80, width: 400, height: 210 }),
            (Left, RectI { x: 120, y: 80, width: 380, height: 200 }),
            (Right, RectI { x: 100, y: 80, width: 420, height: 200 }),
        ] {
            assert_eq!(resized_selection(SELECTION, handle, (103.5, 82.5), (123.5, 92.5), SCREEN), expected);
            assert_eq!(resized_selection(SELECTION, handle, (103.5, 82.5), (103.5, 82.5), SCREEN), SELECTION);
        }
    }

    #[test]
    fn resize_clamps_to_screen_and_never_collapses_or_crosses_opposite_edge() {
        assert_eq!(
            resized_selection(SELECTION, ResizeHandle::TopLeft, (0.0, 0.0), (-2000.0, -2000.0), SCREEN),
            RectI { x: 0, y: 0, width: 500, height: 280 }
        );
        assert_eq!(
            resized_selection(SELECTION, ResizeHandle::BottomRight, (0.0, 0.0), (2000.0, 2000.0), SCREEN),
            RectI { x: 100, y: 80, width: 900, height: 420 }
        );
        assert_eq!(
            resized_selection(SELECTION, ResizeHandle::TopLeft, (0.0, 0.0), (2000.0, 2000.0), SCREEN),
            RectI { x: 499, y: 279, width: 1, height: 1 }
        );
        assert_eq!(
            resized_selection(SELECTION, ResizeHandle::BottomRight, (0.0, 0.0), (-2000.0, -2000.0), SCREEN),
            RectI { x: 100, y: 80, width: 1, height: 1 }
        );
    }

    #[test]
    fn resize_handles_stay_visible_at_screen_edges() {
        for screen in [SCREEN, PixelSize { width: 1, height: 1 }] {
            let selection = RectI { x: 0, y: 0, width: screen.width as i32, height: screen.height as i32 };
            for rect in resize_handle_rects(selection, screen) {
                assert!(rect.x >= 0 && rect.y >= 0);
                assert!(rect.x + rect.width <= screen.width as i32);
                assert!(rect.y + rect.height <= screen.height as i32);
            }
        }
    }
}
