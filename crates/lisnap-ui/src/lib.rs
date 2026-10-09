//! Native desktop shell. Full-screen capture overlays are a separate future platform surface.
mod app;
mod selection;
mod settings;

use std::{
    path::PathBuf,
    sync::{Arc, mpsc},
};

use eframe::egui;
use lisnap_core::{AppSettings, BackendDescriptor, CaptureFrame, CaptureRequest, PixelRect, ScreenInfo};

pub type CaptureResult = Result<Vec<CaptureFrame>, String>;
pub type CaptureReceiver = mpsc::Receiver<CaptureResult>;
pub type CaptureLauncher = Arc<dyn Fn(CaptureRequest) -> CaptureReceiver + Send + Sync>;
pub type ScreenResult = Result<Vec<ScreenInfo>, String>;
pub type ScreenReceiver = mpsc::Receiver<ScreenResult>;
pub type ScreenLauncher = Arc<dyn Fn() -> ScreenReceiver + Send + Sync>;
pub type OverlayResult = Result<Option<PixelRect>, String>;
pub type OverlayReceiver = mpsc::Receiver<OverlayResult>;
pub type OverlayLauncher = Arc<dyn Fn(CaptureFrame) -> OverlayReceiver + Send + Sync>;

pub fn run(
    selected: Option<BackendDescriptor>,
    backends: Vec<BackendDescriptor>,
    capture_launcher: Option<CaptureLauncher>,
    screen_launcher: Option<ScreenLauncher>,
) -> eframe::Result<()> {
    run_with_overlay(selected, backends, capture_launcher, screen_launcher, None)
}

pub fn run_with_overlay(
    selected: Option<BackendDescriptor>,
    backends: Vec<BackendDescriptor>,
    capture_launcher: Option<CaptureLauncher>,
    screen_launcher: Option<ScreenLauncher>,
    overlay_launcher: Option<OverlayLauncher>,
) -> eframe::Result<()> {
    run_with_overlay_and_cursor(selected, backends, capture_launcher, screen_launcher, overlay_launcher, false)
}

pub fn run_with_overlay_and_cursor(
    selected: Option<BackendDescriptor>,
    backends: Vec<BackendDescriptor>,
    capture_launcher: Option<CaptureLauncher>,
    screen_launcher: Option<ScreenLauncher>,
    overlay_launcher: Option<OverlayLauncher>,
    include_cursor: bool,
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
        Box::new(move |_cc| {
            Ok(Box::new(app::LiSnapApp::new_with_overlay_and_cursor(
                selected,
                backends,
                capture_launcher,
                screen_launcher,
                overlay_launcher,
                include_cursor,
            )))
        }),
    )
}

pub fn run_settings(settings: AppSettings, config_path: PathBuf) -> eframe::Result<()> {
    settings::run(settings, config_path)
}
