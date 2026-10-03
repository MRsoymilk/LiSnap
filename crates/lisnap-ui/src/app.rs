use std::{sync::mpsc::TryRecvError, time::Duration};

use eframe::egui;
use lisnap_core::{BackendDescriptor, CaptureCancellation, CaptureFrame, CaptureRequest};

use crate::{CaptureLauncher, CaptureReceiver};

struct ActiveCapture {
    receiver: CaptureReceiver,
    cancellation: CaptureCancellation,
}

pub(crate) struct LiSnapApp {
    selected: Option<BackendDescriptor>,
    backends: Vec<BackendDescriptor>,
    request: CaptureRequest,
    capture_launcher: Option<CaptureLauncher>,
    active_capture: Option<ActiveCapture>,
    captured_frames: Vec<CaptureFrame>,
    preview_texture: Option<egui::TextureHandle>,
    preview_output: Option<String>,
    capture_message: Option<String>,
}

impl LiSnapApp {
    pub(crate) fn new(
        selected: Option<BackendDescriptor>,
        backends: Vec<BackendDescriptor>,
        capture_launcher: Option<CaptureLauncher>,
    ) -> Self {
        Self {
            selected,
            backends,
            request: CaptureRequest::default(),
            capture_launcher,
            active_capture: None,
            captured_frames: Vec::new(),
            preview_texture: None,
            preview_output: None,
            capture_message: None,
        }
    }

    fn start_capture(&mut self) {
        if self.active_capture.is_some() {
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
        self.capture_message = Some("Capturing…".into());
        self.active_capture = Some(ActiveCapture { receiver: launcher(request), cancellation });
    }

    fn cancel_capture(&mut self) {
        if let Some(active) = &self.active_capture {
            active.cancellation.cancel();
            self.capture_message = Some("Cancelling…".into());
        }
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

    fn capture_controls(&mut self, ui: &mut egui::Ui) {
        let busy = self.active_capture.is_some();
        ui.horizontal(|ui| {
            if ui.add_enabled(self.capture_launcher.is_some() && !busy, egui::Button::new("Capture screen")).clicked() {
                self.start_capture();
            }
            if ui.add_enabled(busy, egui::Button::new("Cancel")).clicked() {
                self.cancel_capture();
            }
            ui.add_enabled(false, egui::Button::new("Copy"));
            ui.add_enabled(false, egui::Button::new("Save"));
        });
        ui.add_enabled_ui(!busy, |ui| {
            ui.checkbox(&mut self.request.include_cursor, "Include cursor");
        });
        ui.small("Capture runs on a worker thread. Selection overlay, clipboard and export are not active yet.");
        if let Some(message) = &self.capture_message {
            ui.label(message);
        }

        if !self.captured_frames.is_empty() {
            ui.add_space(8.0);
            ui.strong("Last capture");
            if let Some(texture) = &self.preview_texture {
                if let Some(output) = &self.preview_output {
                    ui.small(format!("Preview: {output}"));
                }
                ui.add(egui::Image::new(texture).max_width(ui.available_width()).max_height(360.0));
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
        self.poll_capture(ui.ctx());
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

    use lisnap_core::{PixelSize, RgbaImage, ScreenInfo};

    use super::*;

    #[test]
    fn overview_renders_without_native_window() {
        let context = egui::Context::default();
        let mut app = LiSnapApp::new(None, Vec::new(), None);

        let mut output = context.run_ui(egui::RawInput::default(), |context| {
            egui::CentralPanel::default().show(context, |ui| app.overview(ui));
        });

        assert!(!output.shapes.is_empty());
        output.textures_delta.clear();
    }

    #[test]
    fn capture_worker_error_returns_to_idle_state() {
        let launcher: CaptureLauncher = Arc::new(|_request| {
            let (sender, receiver) = mpsc::channel();
            sender.send(Err("synthetic failure".into())).unwrap();
            receiver
        });
        let mut app = LiSnapApp::new(None, Vec::new(), Some(launcher));
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
        let mut app = LiSnapApp::new(None, Vec::new(), Some(launcher));
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
        let mut app = LiSnapApp::new(None, Vec::new(), Some(launcher));
        app.start_capture();
        let request = request_receiver.recv().unwrap();
        assert!(!request.is_cancelled());

        app.cancel_capture();

        assert!(request.is_cancelled());
        assert_eq!(app.capture_message.as_deref(), Some("Cancelling…"));
    }
}
