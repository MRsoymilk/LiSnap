//! Standalone preferences window. This never requests screen capture access.
use std::path::PathBuf;

use eframe::egui;
use lisnap_core::AppSettings;

struct SettingsApp {
    settings: AppSettings,
    config_path: PathBuf,
    status: Option<(String, bool)>,
}

impl SettingsApp {
    fn new(settings: AppSettings, config_path: PathBuf) -> Self {
        Self { settings, config_path, status: None }
    }

    fn save(&mut self) {
        self.status = Some(match self.settings.save_to(&self.config_path) {
            Ok(()) => ("Preferences saved. New screenshots will use this setting.".into(), false),
            Err(error) => (error, true),
        });
    }
}

impl eframe::App for SettingsApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let context = ui.ctx().clone();
        let background = egui::Color32::from_rgb(19, 23, 31);
        let card = egui::Color32::from_rgb(29, 35, 45);
        let subtle = egui::Color32::from_rgb(159, 172, 189);
        let accent = egui::Color32::from_rgb(111, 191, 238);
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(background).inner_margin(24))
            .show(ui, |ui| {
                ui.heading(egui::RichText::new("LiSnap Settings").size(24.0).strong());
                ui.add_space(5.0);
                ui.label(egui::RichText::new("Screenshot preferences").color(subtle));
                ui.add_space(20.0);

                egui::Frame::new()
                    .fill(card)
                    .corner_radius(10)
                    .inner_margin(18)
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.label(egui::RichText::new("Capture").size(17.0).strong().color(accent));
                        ui.add_space(12.0);
                        if ui.checkbox(&mut self.settings.include_cursor, "Show mouse cursor in screenshots").changed() {
                            self.save();
                        }
                        ui.add_space(7.0);
                        ui.label(
                            egui::RichText::new(
                                "On: the Wayland compositor draws the pointer into the captured image.\nOff: the screenshot omits the pointer.",
                            )
                            .color(subtle)
                            .size(12.0),
                        );
                    });

                ui.add_space(18.0);
                ui.label(egui::RichText::new("Configuration file").strong());
                ui.add_space(3.0);
                ui.label(egui::RichText::new(self.config_path.display().to_string()).monospace().color(subtle));
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new("Preferences save automatically. --include-cursor and --no-cursor temporarily override them.")
                        .size(12.0)
                        .color(subtle),
                );

                if let Some((message, failed)) = &self.status {
                    ui.add_space(12.0);
                    ui.label(
                        egui::RichText::new(message).color(if *failed {
                            egui::Color32::from_rgb(241, 126, 123)
                        } else {
                            egui::Color32::from_rgb(127, 218, 157)
                        }),
                    );
                    if *failed && ui.button("Retry saving").clicked() {
                        self.save();
                    }
                }

                ui.add_space(20.0);
                ui.horizontal(|ui| {
                    if ui.button("Restore default (hide cursor)").clicked() {
                        self.settings = AppSettings::default();
                        self.save();
                    }
                    if ui.button("Close").clicked() {
                        context.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
            });
    }
}

pub(crate) fn run(settings: AppSettings, config_path: PathBuf) -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_app_id("lisnap-settings")
            .with_inner_size([630.0, 390.0])
            .with_min_inner_size([530.0, 340.0]),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    eframe::run_native(
        "LiSnap Settings",
        options,
        Box::new(move |cc| {
            cc.egui_ctx.set_visuals(egui::Visuals::dark());
            Ok(Box::new(SettingsApp::new(settings, config_path)))
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggles_are_persisted_to_disk() {
        let base = std::env::temp_dir().join(format!(
            "lisnap-ui-settings-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        let path = base.join("config.toml");
        let mut app = SettingsApp::new(AppSettings::default(), path.clone());
        app.settings.include_cursor = true;
        app.save();
        assert!(!app.status.as_ref().unwrap().1);
        assert!(AppSettings::load_from(&path).unwrap().include_cursor);
        app.settings.include_cursor = false;
        app.save();
        assert!(!AppSettings::load_from(&path).unwrap().include_cursor);
        std::fs::remove_dir_all(base).unwrap();
    }
}
