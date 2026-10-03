use std::{sync::mpsc::TryRecvError, time::Duration};

use eframe::egui;
use lisnap_core::{BackendDescriptor, CaptureCancellation, CaptureFrame, CaptureRequest, PixelSize, ScreenInfo};

use crate::{
    CaptureLauncher, CaptureReceiver, OverlayLauncher, OverlayReceiver, ScreenLauncher, ScreenReceiver,
    selection::{normalized_selection, pixels_to_normalized_selection, selection_pixels},
};

struct ActiveCapture {
    receiver: CaptureReceiver,
    cancellation: CaptureCancellation,
}

pub(crate) struct LiSnapApp {
    selected: Option<BackendDescriptor>,
    backends: Vec<BackendDescriptor>,
    request: CaptureRequest,
    capture_launcher: Option<CaptureLauncher>,
    overlay_launcher: Option<OverlayLauncher>,
    overlay_receiver: Option<OverlayReceiver>,
    screen_launcher: Option<ScreenLauncher>,
    screen_receiver: Option<ScreenReceiver>,
    screens: Vec<ScreenInfo>,
    screens_requested: bool,
    screen_message: Option<String>,
    active_capture: Option<ActiveCapture>,
    captured_frames: Vec<CaptureFrame>,
    preview_texture: Option<egui::TextureHandle>,
    preview_output: Option<String>,
    selection_anchor: Option<egui::Pos2>,
    preview_selection: Option<egui::Rect>,
    crop_texture: Option<egui::TextureHandle>,
    crop_size: Option<PixelSize>,
    capture_message: Option<String>,
    overlay_message: Option<String>,
}

impl LiSnapApp {
    pub(crate) fn new_with_overlay(
        selected: Option<BackendDescriptor>,
        backends: Vec<BackendDescriptor>,
        capture_launcher: Option<CaptureLauncher>,
        screen_launcher: Option<ScreenLauncher>,
        overlay_launcher: Option<OverlayLauncher>,
    ) -> Self {
        Self {
            selected,
            backends,
            request: CaptureRequest::default(),
            capture_launcher,
            overlay_launcher,
            overlay_receiver: None,
            screen_launcher,
            screen_receiver: None,
            screens: Vec::new(),
            screens_requested: false,
            screen_message: None,
            active_capture: None,
            captured_frames: Vec::new(),
            preview_texture: None,
            preview_output: None,
            selection_anchor: None,
            preview_selection: None,
            crop_texture: None,
            crop_size: None,
            capture_message: None,
            overlay_message: None,
        }
    }

    fn refresh_screens(&mut self) {
        if self.screen_receiver.is_some() {
            return;
        }
        let Some(launcher) = self.screen_launcher.as_ref() else {
            self.screen_message = Some("Selected backend cannot enumerate outputs.".into());
            return;
        };
        self.screens_requested = true;
        self.screen_message = Some("Refreshing outputs…".into());
        self.screen_receiver = Some(launcher());
    }

    fn poll_screens(&mut self, context: &egui::Context) {
        let Some(result) = self.screen_receiver.as_ref().map(|receiver| receiver.try_recv()) else {
            return;
        };
        match result {
            Ok(Ok(screens)) => {
                self.screen_receiver = None;
                let count = screens.len();
                if count == 0 {
                    self.screens.clear();
                    self.request.screen_id = None;
                    self.screen_message = Some("Backend returned no outputs.".into());
                    return;
                }
                if self
                    .request
                    .screen_id
                    .as_ref()
                    .is_some_and(|selected| !screens.iter().any(|screen| &screen.id == selected))
                {
                    self.request.screen_id = None;
                }
                self.screens = screens;
                self.screen_message = Some(format!("Found {count} output(s)."));
            }
            Ok(Err(error)) => {
                self.screen_receiver = None;
                self.screens.clear();
                self.request.screen_id = None;
                self.screen_message = Some(format!("Output discovery failed: {error}"));
            }
            Err(TryRecvError::Disconnected) => {
                self.screen_receiver = None;
                self.screen_message = Some("Output discovery worker disconnected.".into());
            }
            Err(TryRecvError::Empty) => {
                context.request_repaint_after(Duration::from_millis(50));
            }
        }
    }

    fn start_capture(&mut self) {
        if self.active_capture.is_some() || self.overlay_receiver.is_some() {
            return;
        }
        let Some(launcher) = self.capture_launcher.as_ref() else {
            self.capture_message = Some("Selected backend is not available for desktop capture.".into());
            return;
        };

        let cancellation = CaptureCancellation::default();
        let mut request = self.request.clone();
        request.cancellation = Some(cancellation.clone());
        self.captured_frames.clear();
        self.preview_texture = None;
        self.preview_output = None;
        self.selection_anchor = None;
        self.preview_selection = None;
        self.crop_texture = None;
        self.crop_size = None;
        self.capture_message = Some("Capturing…".into());
        self.active_capture = Some(ActiveCapture { receiver: launcher(request), cancellation });
    }

    fn cancel_capture(&mut self) {
        if let Some(active) = &self.active_capture {
            active.cancellation.cancel();
            self.capture_message = Some("Cancelling…".into());
        }
    }

    fn clear_selection(&mut self) {
        self.selection_anchor = None;
        self.preview_selection = None;
        self.crop_texture = None;
        self.crop_size = None;
    }

    fn poll_capture(&mut self, context: &egui::Context) {
        let Some(result) = self.active_capture.as_ref().map(|active| active.receiver.try_recv()) else {
            return;
        };
        match result {
            Ok(Ok(frames)) => {
                self.active_capture = None;
                if frames.is_empty() {
                    self.capture_message = Some("Capture failed: backend returned no frames.".into());
                } else {
                    let count = frames.len();
                    self.update_preview(context, &frames);
                    self.captured_frames = frames;
                    self.capture_message = Some(format!("Captured {count} output(s)."));
                }
            }
            Ok(Err(error)) => {
                self.active_capture = None;
                self.capture_message = Some(format!("Capture failed: {error}"));
            }
            Err(TryRecvError::Disconnected) => {
                self.active_capture = None;
                self.capture_message = Some("Capture worker disconnected before returning a result.".into());
            }
            Err(TryRecvError::Empty) => {
                context.request_repaint_after(Duration::from_millis(50));
            }
        }
    }

    fn start_overlay(&mut self) {
        if self.overlay_receiver.is_some() || self.active_capture.is_some() {
            return;
        }
        let Some(launcher) = self.overlay_launcher.clone() else {
            self.overlay_message = Some("Wayland selection overlay is not available.".into());
            return;
        };
        let Some(frame) = self.captured_frames.first().cloned() else {
            self.overlay_message = Some("Capture a screen before opening the selection overlay.".into());
            return;
        };
        self.clear_selection();
        self.overlay_message = Some("Selection overlay active — drag to select, Esc to cancel.".into());
        self.overlay_receiver = Some(launcher(frame));
    }

    fn poll_overlay(&mut self, context: &egui::Context) {
        let Some(result) = self.overlay_receiver.as_ref().map(|receiver| receiver.try_recv()) else {
            return;
        };
        match result {
            Ok(Ok(Some(rect))) => {
                self.overlay_receiver = None;
                let Some(frame) = self.captured_frames.first() else {
                    self.overlay_message = Some("Selection returned after the captured frame was cleared.".into());
                    return;
                };
                let Some(selection) = pixels_to_normalized_selection(rect, frame.image.size()) else {
                    self.overlay_message = Some("Overlay returned an invalid crop region.".into());
                    return;
                };
                self.preview_selection = Some(selection);
                match self.build_crop_preview(context) {
                    Ok(()) => {
                        self.overlay_message =
                            Some(format!("Selected {}x{} px at {},{}.", rect.width, rect.height, rect.x, rect.y));
                    }
                    Err(error) => {
                        self.overlay_message = Some(format!("Overlay crop failed: {error}"));
                    }
                }
            }
            Ok(Ok(None)) => {
                self.overlay_receiver = None;
                self.overlay_message = Some("Selection overlay cancelled.".into());
            }
            Ok(Err(error)) => {
                self.overlay_receiver = None;
                self.overlay_message = Some(format!("Selection overlay failed: {error}"));
            }
            Err(TryRecvError::Disconnected) => {
                self.overlay_receiver = None;
                self.overlay_message = Some("Selection overlay worker disconnected.".into());
            }
            Err(TryRecvError::Empty) => {
                context.request_repaint_after(Duration::from_millis(50));
            }
        }
    }

    fn update_preview(&mut self, context: &egui::Context, frames: &[CaptureFrame]) {
        let Some(frame) = frames.first() else {
            self.preview_texture = None;
            self.preview_output = None;
            return;
        };
        let size = frame.image.size();
        let image =
            egui::ColorImage::from_rgba_unmultiplied([size.width as usize, size.height as usize], frame.image.pixels());
        self.preview_texture = Some(context.load_texture("capture-preview", image, egui::TextureOptions::LINEAR));
        self.preview_output = Some(frame.screen.name.clone());
    }

    fn draw_preview(&mut self, ui: &mut egui::Ui) {
        let Some(texture) = self.preview_texture.clone() else {
            return;
        };
        let source_size = texture.size_vec2();
        if source_size.x <= 0.0 || source_size.y <= 0.0 {
            return;
        }
        let max_width = ui.available_width().max(1.0);
        let scale = (max_width / source_size.x).min(360.0 / source_size.y).min(1.0);
        let desired_size = source_size * scale.max(0.01);
        let (rect, response) = ui.allocate_exact_size(desired_size, egui::Sense::click_and_drag());
        ui.painter().image(
            texture.id(),
            rect,
            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );

        if response.drag_started() {
            self.selection_anchor = response
                .interact_pointer_pos()
                .map(|point| egui::pos2(point.x.clamp(rect.min.x, rect.max.x), point.y.clamp(rect.min.y, rect.max.y)));
            self.preview_selection = None;
            self.crop_texture = None;
            self.crop_size = None;
        }
        if response.dragged()
            && let (Some(anchor), Some(pointer)) = (self.selection_anchor, response.interact_pointer_pos())
        {
            self.preview_selection = normalized_selection(rect, anchor, pointer);
        }
        if response.double_clicked() {
            self.clear_selection();
        }

        if let Some(selection) = self.preview_selection {
            let selection_rect = egui::Rect::from_min_max(
                egui::pos2(rect.min.x + selection.min.x * rect.width(), rect.min.y + selection.min.y * rect.height()),
                egui::pos2(rect.min.x + selection.max.x * rect.width(), rect.min.y + selection.max.y * rect.height()),
            );
            ui.painter().rect_filled(selection_rect, 0.0, egui::Color32::from_white_alpha(24));
            let stroke = egui::Stroke::new(1.5, egui::Color32::WHITE);
            let min = selection_rect.min;
            let max = selection_rect.max;
            ui.painter().line_segment([min, egui::pos2(max.x, min.y)], stroke);
            ui.painter().line_segment([egui::pos2(max.x, min.y), max], stroke);
            ui.painter().line_segment([max, egui::pos2(min.x, max.y)], stroke);
            ui.painter().line_segment([egui::pos2(min.x, max.y), min], stroke);
        }
    }

    fn build_crop_preview(&mut self, context: &egui::Context) -> Result<(), String> {
        let selection = self.preview_selection.ok_or("no preview selection")?;
        let frame = self.captured_frames.first().ok_or("no captured frame")?;
        let rect = selection_pixels(selection, frame.image.size()).ok_or("selection has no pixels")?;
        let cropped = frame.image.crop(rect).map_err(|error| error.to_string())?;
        let size = cropped.size();
        let image =
            egui::ColorImage::from_rgba_unmultiplied([size.width as usize, size.height as usize], cropped.pixels());
        self.crop_texture = Some(context.load_texture("crop-preview", image, egui::TextureOptions::LINEAR));
        self.crop_size = Some(size);
        Ok(())
    }

    fn capture_controls(&mut self, ui: &mut egui::Ui) {
        let capture_busy = self.active_capture.is_some();
        let overlay_busy = self.overlay_receiver.is_some();
        let busy = capture_busy || overlay_busy;
        let screen_loading = self.screen_receiver.is_some();

        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    self.screen_launcher.is_some() && !busy && !screen_loading,
                    egui::Button::new("Refresh outputs"),
                )
                .clicked()
            {
                self.refresh_screens();
            }
            ui.add_enabled_ui(!busy, |ui| {
                let selected_text = self
                    .request
                    .screen_id
                    .as_ref()
                    .and_then(|id| self.screens.iter().find(|screen| &screen.id == id))
                    .map_or("All outputs", |screen| screen.name.as_str());
                egui::ComboBox::from_id_salt("capture_output").selected_text(selected_text).show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.request.screen_id, None, "All outputs");
                    for screen in &self.screens {
                        ui.selectable_value(&mut self.request.screen_id, Some(screen.id.clone()), &screen.name);
                    }
                });
            });
        });
        if let Some(message) = &self.screen_message {
            ui.small(message);
        }

        ui.horizontal(|ui| {
            if ui.add_enabled(self.capture_launcher.is_some() && !busy, egui::Button::new("Capture screen")).clicked() {
                self.start_capture();
            }
            if ui.add_enabled(capture_busy, egui::Button::new("Cancel capture")).clicked() {
                self.cancel_capture();
            }
            ui.add_enabled(false, egui::Button::new("Copy"));
            ui.add_enabled(false, egui::Button::new("Save"));
        });
        ui.add_enabled_ui(!busy, |ui| {
            ui.checkbox(&mut self.request.include_cursor, "Include cursor");
        });
        ui.small(
            "Capture and the Wayland selection overlay run on worker threads. Clipboard and export are not active yet.",
        );
        if let Some(message) = &self.capture_message {
            ui.label(message);
        }
        if let Some(message) = &self.overlay_message {
            ui.label(message);
        }

        if !self.captured_frames.is_empty() {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.strong("Last capture");
                if ui
                    .add_enabled(self.overlay_launcher.is_some() && !busy, egui::Button::new("Select region"))
                    .clicked()
                {
                    self.start_overlay();
                }
                if overlay_busy {
                    ui.spinner();
                }
            });
            if let Some(texture) = &self.preview_texture {
                if let Some(output) = &self.preview_output {
                    ui.small(format!("Preview: {output}"));
                }
                let _ = texture;
                self.draw_preview(ui);
                if let Some(selection) = self.preview_selection
                    && let Some(frame) = self.captured_frames.first()
                    && let Some(pixels) = selection_pixels(selection, frame.image.size())
                {
                    ui.horizontal(|ui| {
                        ui.small(format!(
                            "Selection: {}x{} px at {},{} — double-click preview to clear",
                            pixels.width, pixels.height, pixels.x, pixels.y
                        ));
                        if ui.button("Preview crop").clicked()
                            && let Err(error) = self.build_crop_preview(ui.ctx())
                        {
                            self.capture_message = Some(format!("Crop failed: {error}"));
                        }
                    });
                } else {
                    ui.small("Drag on the preview to create a crop selection.");
                }
                if let (Some(texture), Some(size)) = (&self.crop_texture, self.crop_size) {
                    ui.small(format!("Crop preview: {}x{} px", size.width, size.height));
                    ui.add(egui::Image::new(texture).max_width(ui.available_width()).max_height(240.0));
                }
                ui.add_space(6.0);
            }
            egui::Grid::new("capture_results").num_columns(4).striped(true).spacing([20.0, 6.0]).show(ui, |ui| {
                ui.strong("Output");
                ui.strong("Pixels");
                ui.strong("Scale");
                ui.strong("RGBA bytes");
                ui.end_row();
                for frame in &self.captured_frames {
                    let size = frame.image.size();
                    ui.label(&frame.screen.name);
                    ui.label(format!("{}x{}", size.width, size.height));
                    ui.label(frame.screen.scale_factor.map_or_else(|| "unknown".into(), |scale| scale.to_string()));
                    ui.label(frame.image.pixels().len().to_string());
                    ui.end_row();
                }
            });
        }
    }

    fn overview(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("LiSnap");
            ui.label(format!("v{} / Rust", env!("CARGO_PKG_VERSION")));
        });
        ui.label("Cross-platform screenshot tool");
        ui.separator();
        ui.heading("Project status");
        ui.label("Linux/Wayland native capture is implemented with ext-image-copy-capture / wlr-screencopy.");
        ui.label("Capture runs outside the egui repaint callback. No Portal request is made.");
        ui.add_space(12.0);
        ui.group(|ui| {
            ui.strong("Selected backend");
            if let Some(backend) = self.selected {
                ui.label(format!("{} / {}", backend.name, backend.status));
                ui.label(backend.technology);
                ui.small("Selection starts from OS/session hints; capture performs native capability negotiation.");
            } else {
                ui.label("No desktop session detected. Select one with --backend when available.");
            }
        });
        ui.add_space(12.0);
        self.capture_controls(ui);
        ui.add_space(12.0);
        ui.heading("Native backend roadmap");
        egui::Grid::new("backend_inventory").num_columns(3).striped(true).spacing([20.0, 8.0]).show(ui, |ui| {
            ui.strong("Backend");
            ui.strong("Status");
            ui.strong("Native API");
            ui.end_row();
            for backend in &self.backends {
                ui.label(backend.name);
                ui.label(backend.status.to_string());
                ui.label(backend.technology);
                ui.end_row();
            }
        });
        ui.add_space(12.0);
        ui.separator();
        ui.small("Next: complete host Wayland pixel validation, then implement the static selection overlay.");
    }
}

impl eframe::App for LiSnapApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if ui.input(|input| input.key_pressed(egui::Key::Escape))
            && self.active_capture.is_none()
            && self.overlay_receiver.is_none()
        {
            self.clear_selection();
        }
        if !self.screens_requested && self.screen_launcher.is_some() {
            self.refresh_screens();
        }
        self.poll_screens(ui.ctx());
        self.poll_capture(ui.ctx());
        self.poll_overlay(ui.ctx());
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| self.overview(ui));
        });
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{Arc, mpsc},
        time::SystemTime,
    };

    use lisnap_core::{PixelRect, PixelSize, RgbaImage, ScreenInfo};

    use super::*;

    #[test]
    fn overview_renders_without_native_window() {
        let context = egui::Context::default();
        let mut app = LiSnapApp::new_with_overlay(None, Vec::new(), None, None, None);

        let mut output = context.run_ui(egui::RawInput::default(), |context| {
            egui::CentralPanel::default().show(context, |ui| app.overview(ui));
        });

        assert!(!output.shapes.is_empty());
        output.textures_delta.clear();
    }

    #[test]
    fn screen_worker_populates_outputs_and_resets_missing_selection() {
        let launcher: ScreenLauncher = Arc::new(|| {
            let (sender, receiver) = mpsc::channel();
            sender
                .send(Ok(vec![ScreenInfo {
                    id: "DP-1".into(),
                    name: "External display".into(),
                    logical_geometry: None,
                    pixel_size: PixelSize { width: 2560, height: 1440 },
                    scale_factor: Some(1.0),
                }]))
                .unwrap();
            receiver
        });
        let mut app = LiSnapApp::new_with_overlay(None, Vec::new(), None, Some(launcher), None);
        app.request.screen_id = Some("missing-output".into());
        app.refresh_screens();

        app.poll_screens(&egui::Context::default());

        assert_eq!(app.screens.len(), 1);
        assert_eq!(app.screens[0].id, "DP-1");
        assert_eq!(app.request.screen_id, None);
        assert_eq!(app.screen_message.as_deref(), Some("Found 1 output(s)."));
    }

    #[test]
    fn capture_worker_error_returns_to_idle_state() {
        let launcher: CaptureLauncher = Arc::new(|_request| {
            let (sender, receiver) = mpsc::channel();
            sender.send(Err("synthetic failure".into())).unwrap();
            receiver
        });
        let mut app = LiSnapApp::new_with_overlay(None, Vec::new(), Some(launcher), None, None);
        app.start_capture();
        assert!(app.active_capture.is_some());

        app.poll_capture(&egui::Context::default());

        assert!(app.active_capture.is_none());
        assert_eq!(app.capture_message.as_deref(), Some("Capture failed: synthetic failure"));
    }

    #[test]
    fn capture_worker_success_builds_preview_texture() {
        let frame = CaptureFrame {
            screen: ScreenInfo {
                id: "display-1".into(),
                name: "Display 1".into(),
                logical_geometry: None,
                pixel_size: PixelSize { width: 2, height: 1 },
                scale_factor: Some(1.0),
            },
            image: RgbaImage::new(PixelSize { width: 2, height: 1 }, vec![255, 0, 0, 255, 0, 255, 0, 255]).unwrap(),
            captured_at: SystemTime::now(),
        };
        let launcher: CaptureLauncher = Arc::new(move |_request| {
            let (sender, receiver) = mpsc::channel();
            sender.send(Ok(vec![frame.clone()])).unwrap();
            receiver
        });
        let context = egui::Context::default();
        let mut app = LiSnapApp::new_with_overlay(None, Vec::new(), Some(launcher), None, None);
        app.start_capture();

        app.poll_capture(&context);

        assert!(app.active_capture.is_none());
        assert_eq!(app.captured_frames.len(), 1);
        assert!(app.preview_texture.is_some());
        assert_eq!(app.preview_output.as_deref(), Some("Display 1"));
        assert_eq!(app.capture_message.as_deref(), Some("Captured 1 output(s)."));
    }

    #[test]
    fn cancel_marks_shared_request_token() {
        let (request_sender, request_receiver) = mpsc::channel();
        let launcher: CaptureLauncher = Arc::new(move |request| {
            request_sender.send(request).unwrap();
            let (_sender, receiver) = mpsc::channel();
            receiver
        });
        let mut app = LiSnapApp::new_with_overlay(None, Vec::new(), Some(launcher), None, None);
        app.start_capture();
        let request = request_receiver.recv().unwrap();
        assert!(!request.is_cancelled());

        app.cancel_capture();

        assert!(request.is_cancelled());
        assert_eq!(app.capture_message.as_deref(), Some("Cancelling…"));
    }

    #[test]
    fn crop_preview_uses_original_pixels() {
        let image = RgbaImage::new(
            PixelSize { width: 4, height: 2 },
            vec![
                1, 0, 0, 255, 2, 0, 0, 255, 3, 0, 0, 255, 4, 0, 0, 255, 5, 0, 0, 255, 6, 0, 0, 255, 7, 0, 0, 255, 8, 0,
                0, 255,
            ],
        )
        .unwrap();
        let frame = CaptureFrame {
            screen: ScreenInfo {
                id: "display-1".into(),
                name: "Display 1".into(),
                logical_geometry: None,
                pixel_size: image.size(),
                scale_factor: Some(1.0),
            },
            image,
            captured_at: SystemTime::now(),
        };
        let context = egui::Context::default();
        let mut app = LiSnapApp::new_with_overlay(None, Vec::new(), None, None, None);
        app.captured_frames = vec![frame];
        app.preview_selection = Some(egui::Rect::from_min_max(egui::pos2(0.25, 0.0), egui::pos2(0.75, 1.0)));

        app.build_crop_preview(&context).unwrap();

        assert_eq!(app.crop_size, Some(PixelSize { width: 2, height: 2 }));
        assert!(app.crop_texture.is_some());
    }

    #[test]
    fn overlay_worker_selection_builds_crop_preview() {
        let image = RgbaImage::new(
            PixelSize { width: 4, height: 2 },
            vec![
                1, 0, 0, 255, 2, 0, 0, 255, 3, 0, 0, 255, 4, 0, 0, 255, 5, 0, 0, 255, 6, 0, 0, 255, 7, 0, 0, 255, 8, 0,
                0, 255,
            ],
        )
        .unwrap();
        let frame = CaptureFrame {
            screen: ScreenInfo {
                id: "DP-1".into(),
                name: "Display 1".into(),
                logical_geometry: None,
                pixel_size: image.size(),
                scale_factor: Some(1.0),
            },
            image,
            captured_at: SystemTime::now(),
        };
        let launcher: OverlayLauncher = Arc::new(|frame| {
            assert_eq!(frame.screen.id, "DP-1");
            let (sender, receiver) = mpsc::channel();
            sender.send(Ok(Some(PixelRect { x: 1, y: 0, width: 2, height: 2 }))).unwrap();
            receiver
        });
        let context = egui::Context::default();
        let mut app = LiSnapApp::new_with_overlay(None, Vec::new(), None, None, Some(launcher));
        app.captured_frames = vec![frame];

        app.start_overlay();
        assert!(app.overlay_receiver.is_some());
        app.poll_overlay(&context);

        assert!(app.overlay_receiver.is_none());
        assert_eq!(app.crop_size, Some(PixelSize { width: 2, height: 2 }));
        assert!(app.crop_texture.is_some());
        assert_eq!(app.overlay_message.as_deref(), Some("Selected 2x2 px at 1,0."));
        let pixels = selection_pixels(app.preview_selection.unwrap(), PixelSize { width: 4, height: 2 }).unwrap();
        assert_eq!(pixels, PixelRect { x: 1, y: 0, width: 2, height: 2 });
    }

    #[test]
    fn overlay_worker_cancel_returns_to_idle() {
        let launcher: OverlayLauncher = Arc::new(|_frame| {
            let (sender, receiver) = mpsc::channel();
            sender.send(Ok(None)).unwrap();
            receiver
        });
        let frame = CaptureFrame {
            screen: ScreenInfo {
                id: "DP-1".into(),
                name: "Display 1".into(),
                logical_geometry: None,
                pixel_size: PixelSize { width: 1, height: 1 },
                scale_factor: Some(1.0),
            },
            image: RgbaImage::new(PixelSize { width: 1, height: 1 }, vec![0, 0, 0, 255]).unwrap(),
            captured_at: SystemTime::now(),
        };
        let mut app = LiSnapApp::new_with_overlay(None, Vec::new(), None, None, Some(launcher));
        app.captured_frames = vec![frame];

        app.start_overlay();
        app.poll_overlay(&egui::Context::default());

        assert!(app.overlay_receiver.is_none());
        assert_eq!(app.overlay_message.as_deref(), Some("Selection overlay cancelled."));
    }

    #[test]
    fn clear_selection_removes_crop_state() {
        let mut app = LiSnapApp::new_with_overlay(None, Vec::new(), None, None, None);
        app.selection_anchor = Some(egui::pos2(10.0, 20.0));
        app.preview_selection = Some(egui::Rect::from_min_max(egui::pos2(0.1, 0.2), egui::pos2(0.5, 0.6)));
        app.crop_size = Some(PixelSize { width: 100, height: 80 });

        app.clear_selection();

        assert!(app.selection_anchor.is_none());
        assert!(app.preview_selection.is_none());
        assert!(app.crop_texture.is_none());
        assert!(app.crop_size.is_none());
    }
}
