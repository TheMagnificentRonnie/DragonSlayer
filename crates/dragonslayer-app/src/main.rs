mod app;
mod icon;
mod images;
mod recent;
mod session;
mod theme;

use std::path::PathBuf;
use std::sync::Arc;

use dragonslayer_camera::{mock::MockBackend, CameraBackend};

fn main() -> eframe::Result {
    let mut mock = false;
    let mut project: Option<PathBuf> = None;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--mock" => mock = true,
            "-h" | "--help" => {
                println!("usage: dragonslayer-app [--mock] [PROJECT_FOLDER]");
                return Ok(());
            }
            _ => project = Some(arg.into()),
        }
    }
    let backend: Option<Box<dyn CameraBackend>> =
        if mock { Some(Box::new(MockBackend::default())) } else { dragonslayer_camera::default_backend() };

    let icon_data = Arc::new(eframe::egui::IconData {
        rgba: icon::rgba(),
        width: icon::WIDTH,
        height: icon::HEIGHT,
    });

    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("DragonSlayer")
            .with_inner_size([1400.0, 900.0])
            .with_min_inner_size([980.0, 620.0])
            .with_icon(icon_data),
        ..Default::default()
    };
    eframe::run_native(
        "DragonSlayer",
        options,
        Box::new(move |cc| {
            theme::install(&cc.egui_ctx, theme::ThemeChoice::DarkTeal);
            Ok(Box::new(app::DragonSlayerApp::new(cc, backend, project)))
        }),
    )
}
