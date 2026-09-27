//! Small helpers shared across the app.

use super::*;

pub(super) fn overlay(ui: &egui::Ui, rect: Rect, scene: &str, mode: &str) {
    let p = ui.painter();
    let font = FontId::proportional(15.0);
    let shadow = Color32::from_black_alpha(160);
    for (pos, align, text) in [
        (rect.left_top() + Vec2::new(10.0, 8.0), Align2::LEFT_TOP, scene),
        (rect.right_top() + Vec2::new(-10.0, 8.0), Align2::RIGHT_TOP, mode),
    ] {
        if text.is_empty() {
            continue;
        }
        p.text(pos + Vec2::splat(1.0), align, text, font.clone(), shadow);
        p.text(pos, align, text, font.clone(), Color32::WHITE);
    }
}

/// UV sub-rectangle that makes an image of `size` cover `area` completely, cropping
/// whichever edges overhang (centred). The counterpart of `fit`.
pub(super) fn cover_uv(area: Rect, size: Vec2) -> Rect {
    let full = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    if size.x <= 0.0 || size.y <= 0.0 || area.width() <= 0.0 || area.height() <= 0.0 {
        return full;
    }
    let scale = (area.width() / size.x).max(area.height() / size.y);
    let visible = Vec2::new(area.width() / scale / size.x, area.height() / scale / size.y);
    Rect::from_center_size(egui::pos2(0.5, 0.5), visible)
}

/// Largest rect with `size`'s aspect ratio centred in `area`.
pub(super) fn fit(area: Rect, size: Vec2) -> Rect {
    if size.x <= 0.0 || size.y <= 0.0 {
        return area;
    }
    let scale = (area.width() / size.x).min(area.height() / size.y);
    Rect::from_center_size(area.center(), size * scale)
}

pub(super) fn connect_hint() -> &'static str {
    if cfg!(windows) {
        "Turn the camera on, set it to PC/PTP mode and plug it in. First time on Windows? Run Zadig (see README)."
    } else if cfg!(target_os = "macos") {
        "Turn the camera on and plug it in. Quit Photos and Image Capture; if it still isn't found, run `killall ptpcamerad`."
    } else {
        "Turn the camera on, set it to PC/PTP mode and plug it in."
    }
}

pub(super) fn human_secs(secs: f32) -> String {
    let s = secs.round() as u64;
    if s < 60 { format!("{s}s") } else { format!("{}m {:02}s", s / 60, s % 60) }
}

/// Windows-only: launch bundled Zadig if it's next to our exe, else open the download page.
/// Zadig itself asks for admin (UAC) and does the WinUSB install. This is a stepping stone
/// to a fully in-app installer using libwdi.
#[cfg(windows)]
pub(super) fn launch_driver_setup() {
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let zadig = dir.join("zadig.exe");
        if zadig.exists() {
            let _ = std::process::Command::new(&zadig).spawn();
            return;
        }
    }
    let _ = std::process::Command::new("cmd")
        .args(["/c", "start", "", "https://zadig.akeo.ie"])
        .spawn();
}

#[cfg(not(windows))]
pub(super) fn launch_driver_setup() {}

/// Simple debug log to %TEMP%\dragonslayer.log (Windows) or ~/Library/Logs/dragonslayer.log (macOS)
/// so we can see what an off-machine build actually does.
pub(super) fn log_line(msg: &str) {
    use std::io::Write;
    if cfg!(test) {
        return;
    }
    #[cfg(target_os = "macos")]
    let dir = std::env::var("HOME").map(|h| std::path::PathBuf::from(h).join("Library/Logs"));
    #[cfg(not(target_os = "macos"))]
    let dir = std::env::var("TEMP").map(std::path::PathBuf::from);
    let Ok(mut dir) = dir else { return };
    dir.push("dragonslayer.log");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&dir) {
        let _ = writeln!(f, "{} {msg}", chrono_stamp());
    }
}

pub(super) fn chrono_stamp() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    let s = now % 60;
    let m = (now / 60) % 60;
    let h = (now / 3600) % 24;
    format!("[{h:02}:{m:02}:{s:02}]")
}

pub(super) fn reveal(path: &Path) {
    let _ = if cfg!(windows) {
        std::process::Command::new("explorer").arg(format!("/select,{}", path.display())).spawn()
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg("-R").arg(path).spawn()
    } else {
        std::process::Command::new("xdg-open").arg(path.parent().unwrap_or(path)).spawn()
    };
}
