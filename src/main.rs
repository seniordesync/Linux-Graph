mod app;
mod graph;
mod package_manager;

use app::LinuxGraphApp;
use eframe::egui;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1440.0, 900.0])
            .with_title("Linux Graph"),
        ..Default::default()
    };

    eframe::run_native(
        "Linux Graph",
        options,
        Box::new(|cc| Ok(Box::new(LinuxGraphApp::new(cc)))),
    )
}
