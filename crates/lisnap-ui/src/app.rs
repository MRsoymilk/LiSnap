use eframe::egui;
use lisnap_core::{BackendDescriptor, CaptureRequest};

pub(crate) struct LiSnapApp {
    selected: Option<BackendDescriptor>,
    backends: Vec<BackendDescriptor>,
    request: CaptureRequest,
}

impl LiSnapApp {
    pub(crate) fn new(selected: Option<BackendDescriptor>, backends: Vec<BackendDescriptor>) -> Self {
        Self { selected, backends, request: CaptureRequest::default() }
    }

    fn overview(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("LiSnap");
            ui.label(format!("v{} / Rust", env!("CARGO_PKG_VERSION")));
        });
        ui.label("Cross-platform screenshot tool");
        ui.separator();
        ui.heading("Project framework");
        ui.label("This is the desktop shell. Native screen capture is not implemented yet.");
        ui.label("No desktop image has been captured. No Portal request is made.");
        ui.add_space(12.0);
        ui.group(|ui| {
            ui.strong("Selected backend");
            if let Some(backend) = self.selected {
                ui.label(format!("{} / {}", backend.name, backend.status));
                ui.label(backend.technology);
                ui.small("Selection is based on OS/session hints, not a successful capability probe.");
            } else {
                ui.label("No desktop session detected. Select one with --backend when available.");
            }
        });
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            ui.add_enabled(false, egui::Button::new("Capture screen"));
            ui.add_enabled(false, egui::Button::new("Copy"));
            ui.add_enabled(false, egui::Button::new("Save"));
        });
        ui.checkbox(&mut self.request.include_cursor, "Include cursor (setting preview)");
        ui.small("Capture, selection overlay, clipboard and export are planned, not active controls.");
        ui.add_space(12.0);
        ui.heading("Native backend roadmap");
        egui::Grid::new("backend_inventory").num_columns(3).striped(true).spacing([20.0, 8.0]).show(ui, |ui| {
            ui.strong("Backend");
            ui.strong("Status");
            ui.strong("Planned API");
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
        ui.small("Next: native Wayland capability discovery and single-output frame capture.");
    }
}

impl eframe::App for LiSnapApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| self.overview(ui));
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overview_renders_without_native_window() {
        let context = egui::Context::default();
        let mut app = LiSnapApp::new(None, Vec::new());

        let mut output = context.run_ui(egui::RawInput::default(), |context| {
            egui::CentralPanel::default().show(context, |ui| app.overview(ui));
        });

        assert!(!output.shapes.is_empty());
        output.textures_delta.clear();
    }
}
