mod app;
mod images;
mod session;

use std::path::PathBuf;

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

    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("DragonSlayer")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([820.0, 520.0]),
        ..Default::default()
    };
    eframe::run_native("DragonSlayer", options, Box::new(move |cc| Ok(Box::new(app::DragonSlayerApp::new(cc, backend, project)))))
}
