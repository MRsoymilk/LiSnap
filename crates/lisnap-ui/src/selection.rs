use eframe::egui;
use lisnap_core::{LogicalRect, PixelRect, PixelSize, selection_to_pixels};

fn clamp_to_rect(point: egui::Pos2, rect: egui::Rect) -> egui::Pos2 {
    egui::pos2(point.x.clamp(rect.min.x, rect.max.x), point.y.clamp(rect.min.y, rect.max.y))
}

pub(crate) fn normalized_selection(rect: egui::Rect, start: egui::Pos2, end: egui::Pos2) -> Option<egui::Rect> {
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return None;
    }
    let start = clamp_to_rect(start, rect);
    let end = clamp_to_rect(end, rect);
    let normalized = egui::Rect::from_two_pos(
        egui::pos2((start.x - rect.min.x) / rect.width(), (start.y - rect.min.y) / rect.height()),
        egui::pos2((end.x - rect.min.x) / rect.width(), (end.y - rect.min.y) / rect.height()),
    );
    (normalized.width() > 0.0 && normalized.height() > 0.0).then_some(normalized)
}

pub(crate) fn selection_pixels(selection: egui::Rect, size: PixelSize) -> Option<PixelRect> {
    selection_to_pixels(
        LogicalRect {
            x: f64::from(selection.min.x),
            y: f64::from(selection.min.y),
            width: f64::from(selection.width()),
            height: f64::from(selection.height()),
        },
        LogicalRect { x: 0.0, y: 0.0, width: 1.0, height: 1.0 },
        size,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_reverse_drag_and_clamps_bounds() {
        let rect = egui::Rect::from_min_max(egui::pos2(100.0, 50.0), egui::pos2(500.0, 250.0));
        let selection = normalized_selection(rect, egui::pos2(450.0, 220.0), egui::pos2(50.0, 20.0)).unwrap();
        assert!((selection.min.x - 0.0).abs() < f32::EPSILON);
        assert!((selection.min.y - 0.0).abs() < f32::EPSILON);
        assert!((selection.max.x - 0.875).abs() < f32::EPSILON);
        assert!((selection.max.y - 0.85).abs() < f32::EPSILON);
    }

    #[test]
    fn maps_normalized_selection_to_pixels() {
        let selection = egui::Rect::from_min_max(egui::pos2(0.25, 0.25), egui::pos2(0.75, 0.75));
        let pixels = selection_pixels(selection, PixelSize { width: 1920, height: 1080 }).unwrap();
        assert_eq!(pixels, PixelRect { x: 480, y: 270, width: 960, height: 540 });
    }
}
