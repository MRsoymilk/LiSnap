//! Native desktop shell. Full-screen capture overlays are a separate future platform surface.
mod app;

use std::sync::{Arc, mpsc};

use eframe::egui;
use lisnap_core::{BackendDescriptor, CaptureFrame, CaptureRequest};

pub type CaptureResult = Result<Vec<CaptureFrame>, String>;
pub type CaptureReceiver = mpsc::Receiver<CaptureResult>;
pub type CaptureLauncher = Arc<dyn Fn(CaptureRequest) -> CaptureReceiver + Send + Sync>;

pub fn run(
    selected: Option<BackendDescriptor>,
    backends: Vec<BackendDescriptor>,
    capture_launcher: Option<CaptureLauncher>,
) -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_app_id("lisnap")
            .with_inner_size([880.0, 540.0])
            .with_min_inner_size([640.0, 420.0]),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    eframe::run_native(
        "LiSnap",
        options,
        Box::new(move |_cc| Ok(Box::new(app::LiSnapApp::new(selected, backends, capture_launcher)))),
    )
}
