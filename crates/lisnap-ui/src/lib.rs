//! Native desktop shell. Full-screen capture overlays are a separate future platform surface.
mod app;

use eframe::egui;
use lisnap_core::BackendDescriptor;

pub fn run(selected: Option<BackendDescriptor>, backends: Vec<BackendDescriptor>) -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_app_id("lisnap")
            .with_inner_size([880.0, 540.0])
            .with_min_inner_size([640.0, 420.0]),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    eframe::run_native("LiSnap", options, Box::new(move |_cc| Ok(Box::new(app::LiSnapApp::new(selected, backends)))))
}
