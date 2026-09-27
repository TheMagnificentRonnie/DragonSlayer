use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align, Align2, Color32, FontId, Key, Layout, Margin, Rect, RichText, Sense, Stroke,
    TextureHandle, TextureOptions, Vec2,
};
use egui_dock::{DockArea, DockState, NodeIndex};
use dragonslayer_core::compile::{self, Format, Framing, Resolution, Settings};
use dragonslayer_core::{Frame, Project};

/// Dockable panes. Users can drag them into new tab groups, resize, or undock.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Tab {
    Scenes,
    Viewer,
    Timeline,
    Camera,
    Exposure,
    Onion,
    Export,
}

impl Tab {
    fn default_layout() -> DockState<Tab> {
        let mut dock = DockState::new(vec![Tab::Viewer]);
        let surface = dock.main_surface_mut();
        // Right side: camera, exposure, onion and export as tabs of one node.
        let [center, _right] = surface.split_right(
            NodeIndex::root(),
            0.72,
            vec![Tab::Camera, Tab::Exposure, Tab::Onion, Tab::Export],
        );
        // Left side: scenes list.
        let [center_after_left, _left] = surface.split_left(center, 0.22, vec![Tab::Scenes]);
        // Bottom: timeline strip.
        let _ = surface.split_below(center_after_left, 0.78, vec![Tab::Timeline]);
        dock
    }
}

use dragonslayer_camera::diag::UsbCamera;

type UsbScan = Result<Vec<UsbCamera>, String>;

mod compile_ui;
mod diagnose;
mod help;
mod import_ui;
mod interval;
mod keys;
mod menu;
mod playback;
mod scenes;
#[cfg(test)]
mod tests;
mod tabs;
mod util;
mod viewer;
mod welcome;

use util::*;

use crate::images::Images;
use crate::session::{Cmd, Event, Session, Status};

const THUMB_WIDTH: u32 = 160;
const DEFAULT_ONION_WIDTH: u32 = 1280;

struct SceneRow {
    id: String,
    name: String,
    fps: Option<u32>,
    count: usize,
    last_jpeg: Option<PathBuf>,
}

#[derive(Clone, Copy, PartialEq)]
enum Scope {
    Scene,
    Project,
}

struct CompileDialog {
    open: bool,
    scope: Scope,
    format: Format,
    resolution: Resolution,
    framing: Framing,
    /// None = use the project (and per-scene) frame rates as-is.
    /// Some(n) = compile at n fps: every scene's per-frame duration becomes 1/n,
    /// so the whole film plays back at that rate regardless of what it was captured at.
    fps: Option<u32>,
    running: Option<Receiver<Result<compile::Output, String>>>,
    /// 0.0–1.0 as f32 bits, written by the compile thread.
    progress: Arc<AtomicU32>,
    started: Instant,
    result: Option<Result<compile::Output, String>>,
}

impl Default for CompileDialog {
    fn default() -> Self {
        Self {
            open: false,
            scope: Scope::Project,
            format: Format::H264,
            resolution: Resolution::Source,
            framing: Framing::Fit,
            fps: None,
            running: None,
            progress: Arc::new(AtomicU32::new(0)),
            started: Instant::now(),
            result: None,
        }
    }
}

pub struct DragonSlayerApp {
    project: Option<Project>,
    scenes: Vec<SceneRow>,
    frames: Vec<Frame>,

    session: Session,
    status: Status,
    live: Option<TextureHandle>,
    mode: Mode,
    /// Last live-active state we sent to the camera worker. `None` means we haven't
    /// sent anything yet; we send on the first frame to establish state.
    want_live_cached: Option<bool>,
    dock_state: DockState<Tab>,
    theme_choice: crate::theme::ThemeChoice,
    /// Wall-clock time of the last playback frame advance; also used as a repaint anchor.
    playback_tick: Instant,
    capturing: bool,
    /// Interval-capture UI settings. Persist across single captures so the user's
    /// last settings are still there when they open the panel again.
    interval_count: u32,
    interval_secs: f64,
    /// Running interval sequence, if any. `None` when idle.
    interval: Option<Interval>,
    /// Camera settings as last reported by the camera. Empty when none are available.
    camera_settings: Vec<dragonslayer_camera::Setting>,
    /// A change sent to the camera that hasn't been confirmed by a re-read yet.
    setting_pending: bool,

    onion_on: bool,
    onion_count: usize,
    onion_opacity: f32,
    onion_edges: bool,
    /// Show the "other view" picture-in-picture in the viewer corner.
    pip_on: bool,
    /// Scale the picture to fill the viewer, cropping edges, instead of letterboxing.
    fill_viewer: bool,
    /// Minimal view: full-screen live view with a small floating control bar.
    minimal: bool,
    /// Whether the window is currently full screen on our account (tracks `minimal`).
    fullscreen_applied: bool,
    /// Pointer was over the minimal-view bar last frame (keeps it from fading).
    minimal_bar_hovered: bool,
    /// Mode as of the end of the previous frame, so mode switches can be logged.
    logged_mode_is_capture: bool,

    images: Images,
    renaming: Option<(String, String)>,
    drag_from: Option<usize>,
    compile: CompileDialog,
    help_open: bool,
    help_os: HelpOs,
    help_tab: HelpTab,
    /// Latest USB device check (Windows): which driver has the camera, is it behind a hub.
    usb_scan: Option<UsbScan>,
    usb_scan_rx: Option<Receiver<Option<UsbScan>>>,
    usb_scan_at: Option<Instant>,
    diagnose_open: bool,
    import: import_ui::ImportDialog,
    recent: crate::recent::Recent,
    /// What the welcome screen offers to reopen; built once at startup.
    welcome_recent: Vec<RecentProject>,
    /// The previous session never closed normally (crash, forced close, power cut).
    last_session_crashed: bool,
    /// Background search for projects on disk, while it runs.
    project_scan: Option<Receiver<Vec<PathBuf>>>,
    /// When the last live view frame arrived, so the diagnosis can tell if live view is flowing.
    last_live_at: Option<Instant>,
    /// Last frame shown in Preview, kept on screen while the next one decodes.
    preview_hold: Option<TextureHandle>,
    message: Option<(String, bool, Instant)>,
    keys: Vec<Key>,
    _awake: Option<keepawake::KeepAwake>,
}

#[derive(Clone, Copy, PartialEq)]
enum HelpOs {
    Windows,
    Mac,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum HelpTab {
    Guide,
    Advanced,
}

/// An interval-capture sequence in progress. Between shots the app waits until
/// `next_at`, then triggers one capture and clears `next_at` until the shot's
/// `Event::Captured` comes back. That way we never overlap: a slow camera just
/// slides the whole sequence, we don't queue captures on top of each other.
#[derive(Clone, Copy, Debug)]
struct Interval {
    /// Total frames the user asked for.
    total: u32,
    /// Frames finished so far (advanced on `Event::Captured(Ok)`).
    done: u32,
    /// Seconds between the end of one capture and the trigger of the next.
    every: f64,
    /// When the next capture should fire. `None` while a capture is in flight.
    next_at: Option<Instant>,
}

/// Two modes: Capture (live view, Space captures) and Preview (browse the
/// captured frames of the active scene with arrows / play / timeline).
#[derive(Clone, Copy, PartialEq, Debug)]
enum Mode {
    Capture,
    Preview { index: usize, playing: bool, last_advance: Instant },
}

impl Mode {
    fn is_capture(self) -> bool {
        matches!(self, Mode::Capture)
    }
    fn is_playing(self) -> bool {
        matches!(self, Mode::Preview { playing: true, .. })
    }
    fn preview_index(self) -> Option<usize> {
        match self {
            Mode::Preview { index, .. } => Some(index),
            _ => None,
        }
    }
}

impl DragonSlayerApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        backend: Option<Box<dyn dragonslayer_camera::CameraBackend>>,
        project: Option<PathBuf>,
    ) -> Self {
        log_line("--- dragonslayer-app starting ---");
        log_line(&format!(
            "exe: {}",
            std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default()
        ));
        log_line(&format!("cwd: {}", std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default()));
        let ctx = cc.egui_ctx.clone();
        let mut app = Self {
            project: None,
            scenes: Vec::new(),
            frames: Vec::new(),
            session: Session::start(backend, ctx.clone()),
            status: Status::Searching,
            live: None,
            mode: Mode::Capture,
            want_live_cached: None,
            dock_state: Tab::default_layout(),
            theme_choice: crate::theme::ThemeChoice::DarkTeal,
            playback_tick: Instant::now(),
            capturing: false,
            interval_count: 10,
            interval_secs: 2.0,
            interval: None,
            camera_settings: Vec::new(),
            setting_pending: false,
            onion_on: true,
            onion_count: 1,
            onion_opacity: 0.55,
            onion_edges: true,
            pip_on: false,
            fill_viewer: true,
            minimal: false,
            fullscreen_applied: false,
            minimal_bar_hovered: false,
            logged_mode_is_capture: true,
            images: Images::new(&ctx),
            help_open: false,
            help_os: if cfg!(target_os = "macos") { HelpOs::Mac } else { HelpOs::Windows },
            help_tab: HelpTab::Guide,
            usb_scan: None,
            usb_scan_rx: None,
            usb_scan_at: None,
            diagnose_open: false,
            import: import_ui::ImportDialog::default(),
            recent: crate::recent::Recent::default(),
            welcome_recent: Vec::new(),
            last_session_crashed: false,
            project_scan: None,
            last_live_at: None,
            preview_hold: None,
            renaming: None,
            drag_from: None,
            compile: CompileDialog::default(),
            message: None,
            keys: Vec::new(),
            _awake: None,
        };
        app.recent = crate::recent::Recent::load();
        app.last_session_crashed = !app.recent.clean_exit;
        app.rebuild_projects_list();
        if app.last_session_crashed {
            log_line("previous session did not close cleanly");
        }
        if let Some(p) = project {
            app.open_project(&p);
        }
        app
    }

    // ---- state ----------------------------------------------------------

    fn info(&mut self, msg: impl Into<String>) {
        self.message = Some((msg.into(), false, Instant::now()));
    }

    fn error(&mut self, msg: impl std::fmt::Display) {
        self.message = Some((msg.to_string(), true, Instant::now()));
    }

    fn open_project(&mut self, path: &Path) {
        match Project::open(path) {
            Ok(p) => {
                match p.recover() {
                    Ok(r) if !r.recovered.is_empty() || !r.abandoned.is_empty() => self.info(format!(
                        "Recovered {} interrupted capture(s); {} had no files (check the camera card).",
                        r.recovered.len(),
                        r.abandoned.len()
                    )),
                    Ok(_) => self.info(format!("Opened {}", p.name())),
                    Err(e) => self.error(e),
                }
                self.set_project(p);
            }
            Err(e) => self.error(format!("Couldn't open {}: {e}", path.display())),
        }
    }

    fn set_project(&mut self, p: Project) {
        self.recent.opened(&p.root);
        self.rebuild_projects_list();
        self.last_session_crashed = false;
        self.project = Some(p);
        self.mode = Mode::Capture;
        self._awake = keepawake::Builder::default()
            .display(true)
            .idle(true)
            .reason("DragonSlayer capture session")
            .app_name("DragonSlayer")
            .create()
            .ok();
        self.refresh();
    }

    fn refresh(&mut self) {
        let Some(p) = &self.project else { return };
        let result = (|| {
            let mut rows = Vec::new();
            for s in p.scenes()? {
                let frames = s.frames()?;
                rows.push(SceneRow {
                    id: s.id().into(),
                    name: s.name().into(),
                    fps: s.file.fps,
                    count: frames.len(),
                    last_jpeg: frames.last().and_then(|f| f.jpeg().map(Path::to_path_buf)),
                });
            }
            let frames = match p.active_scene() {
                Ok(s) => s.frames()?,
                Err(_) => Vec::new(),
            };
            Ok::<_, dragonslayer_core::Error>((rows, frames))
        })();
        match result {
            Ok((rows, frames)) => {
                self.scenes = rows;
                self.frames = frames;
                // A different scene or a deleted frame: don't hold a stale picture.
                self.preview_hold = None;
                // The frame list can shrink (delete last, switching to a shorter scene), so
                // pull a Preview cursor back in range before anything indexes with it.
                if let Mode::Preview { index, .. } = &mut self.mode {
                    match self.frames.len().checked_sub(1) {
                        Some(last) => *index = (*index).min(last),
                        None => self.mode = Mode::Capture,
                    }
                }
            }
            Err(e) => self.error(e),
        }
    }

    /// Runs a project mutation, then refreshes the cached view of it.
    fn edit(&mut self, f: impl FnOnce(&mut Project) -> dragonslayer_core::Result<()>) {
        let Some(p) = &mut self.project else { return };
        if let Err(e) = f(p) {
            self.error(e);
        }
        self.refresh();
    }

    fn active_row(&self) -> Option<&SceneRow> {
        let id = self.project.as_ref()?.file.active_scene.as_deref()?;
        self.scenes.iter().find(|r| r.id == id)
    }

    fn camera_ready(&self) -> bool {
        matches!(self.status, Status::Connected { .. })
    }

    fn has_live_view(&self) -> bool {
        matches!(self.status, Status::Connected { caps, .. } if caps.live_view)
    }

    fn capture(&mut self) {
        if !self.mode.is_capture() {
            self.info("Preview mode: switch to Capture (Tab) to take frames");
            return;
        }
        // An import appends to scenes too; two writers could hand out the same frame number.
        if self.capturing || !self.camera_ready() || self.import.is_running() {
            return;
        }
        if let Some(p) = &self.project
            && self.session.cmd.send(Cmd::Capture(p.clone())).is_ok() {
                self.capturing = true;
            }
    }

    fn delete_last(&mut self) {
        let Some(p) = &self.project else { return };
        let result = p.active_scene().and_then(|s| s.delete_last());
        match result {
            Ok(frame) => self.info(format!("Frame {frame} moved to the scene's trash folder")),
            Err(e) => self.error(e),
        }
        self.refresh();
    }

    fn onion_width(&self) -> u32 {
        self.live.as_ref().map_or(DEFAULT_ONION_WIDTH, |t| t.size()[0] as u32)
    }

    fn handle_events(&mut self, ctx: &egui::Context) {
        while let Ok(ev) = self.session.events.try_recv() {
            match ev {
                Event::Status(s) => {
                    if !matches!(s, Status::Connected { .. }) {
                        self.live = None;
                        self.capturing = false;
                        if self.interval.is_some() {
                            self.stop_interval("camera lost");
                        }
                        self.camera_settings.clear();
                        self.setting_pending = false;
                    }
                    self.status = s;
                }
                Event::Live(img) => {
                    self.last_live_at = Some(Instant::now());
                    match &mut self.live {
                        Some(t) if t.size() == img.size => t.set(img, TextureOptions::LINEAR),
                        _ => self.live = Some(ctx.load_texture("live view", img, TextureOptions::LINEAR)),
                    }
                }
                Event::Settings(settings) => {
                    self.camera_settings = settings;
                    self.setting_pending = false;
                }
                Event::SettingFailed(msg) => self.error(msg),
                Event::Captured(result) => {
                    self.capturing = false;
                    match result {
                        Ok(c) => {
                            self.refresh();
                            let width = self.onion_width();
                            let jpeg = c.files.iter().find(|f| dragonslayer_core::scene::is_jpeg(f));
                            if let Some(jpg) = jpeg {
                                self.images.prefetch(jpg, width);
                                self.images.prefetch(jpg, THUMB_WIDTH);
                            }
                            if jpeg.is_none() {
                                // Said loudly, every time: RAW-only frames can't be seen or compiled.
                                self.error(format!(
                                    "Frame {} is RAW only: it can't be shown or compiled. Set Image format to \
                                     RAW + JPEG in the Exposure panel (or on the camera).",
                                    c.frame
                                ));
                            }
                            if let Some(iv) = self.interval.as_mut() {
                                iv.done = iv.done.saturating_add(1);
                                if iv.done >= iv.total {
                                    let total = iv.total;
                                    self.interval = None;
                                    self.info(format!("Interval done: {total} frames captured"));
                                } else {
                                    iv.next_at = Some(Instant::now() + Duration::from_secs_f64(iv.every.max(0.0)));
                                    let done = iv.done;
                                    let total = iv.total;
                                    if jpeg.is_some() {
                                        self.info(format!("Interval: {done}/{total}"));
                                    }
                                }
                            } else if jpeg.is_some() {
                                let raw = c.files.iter().any(|f| !dragonslayer_core::scene::is_jpeg(f));
                                if raw {
                                    self.info(format!("Frame {} saved", c.frame));
                                } else {
                                    self.info(format!(
                                        "Frame {} saved (JPEG only: set RAW+JPEG on the camera to keep RAW)",
                                        c.frame
                                    ));
                                }
                            }
                        }
                        Err(e) => {
                            if self.interval.is_some() {
                                self.stop_interval(&format!("capture failed: {e}"));
                            } else {
                                self.error(format!("Capture failed: {e}"));
                            }
                        }
                    }
                }
            }
        }

        if let Some(rx) = &self.usb_scan_rx
            && let Ok(result) = rx.try_recv()
        {
            self.usb_scan_rx = None;
            self.usb_scan = result;
        }

        if let Some(rx) = &self.compile.running
            && let Ok(result) = rx.try_recv() {
                self.compile.running = None;
                self.compile.result = Some(result);
            }
    }

}

/// A recent project as shown on the welcome screen.
struct RecentProject {
    path: PathBuf,
    /// `None` when the folder can't be opened (moved, deleted, drive unplugged).
    name: Option<String>,
    scene: Option<String>,
    frames: usize,
    last_jpeg: Option<PathBuf>,
}

impl eframe::App for DragonSlayerApp {
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        self.take_shortcuts(raw, ctx.text_edit_focused());
    }

    fn on_exit(&mut self) {
        self.recent.closed_cleanly();
        log_line("closed cleanly");
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.images.begin_frame(&ctx);
        if self.mode.is_capture() != self.logged_mode_is_capture {
            self.logged_mode_is_capture = self.mode.is_capture();
            log_line(&format!("mode -> {}", if self.logged_mode_is_capture { "Capture" } else { "Preview" }));
        }
        self.handle_events(&ctx);
        self.poll_project_scan();
        self.handle_keys();
        if self.project.is_none() {
            self.minimal = false;
        }
        if self.minimal && !self.mode.is_capture() {
            self.mode = Mode::Capture;
        }
        if self.minimal != self.fullscreen_applied {
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.minimal));
            self.fullscreen_applied = self.minimal;
            log_line(if self.minimal { "minimal view on" } else { "minimal view off" });
        }
        // Panasonic PTP wedges under a continuous stream of preview requests.
        // Tell the camera worker to keep live view running only while we're in
        // Capture mode; pause it when we're browsing captured frames.
        let want_live = self.mode.is_capture();
        if self.want_live_cached != Some(want_live) {
            let _ = self.session.cmd.send(crate::session::Cmd::SetLiveActive(want_live));
            self.want_live_cached = Some(want_live);
        }

        if !self.minimal {
            egui::Panel::top("menubar").show(ui, |ui| self.menu_bar(ui));
            egui::Panel::top("statusbar").show(ui, |ui| {
                ui.add_space(2.0);
                self.status_bar(ui);
                ui.add_space(2.0);
            });
        }

        self.auto_usb_scan(&ctx);

        if self.project.is_none() {
            egui::CentralPanel::default().show(ui, |ui| self.welcome(ui));
            self.help_window(&ctx);
            self.diagnose_window(&ctx);
            return;
        }

        // Real dockable panels. Users can drag tabs into new groups, resize the
        // splits, or drop a tab back to reset. Layout persists for the session.
        if self.minimal {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE.fill(Color32::BLACK))
                .show(ui, |ui| self.viewer(ui));
            self.minimal_bar(&ctx);
            // Keep the bar's fade-out animating.
            ctx.request_repaint_after(Duration::from_millis(100));
        } else {
            let mut dock = std::mem::replace(&mut self.dock_state, DockState::new(vec![]));
            egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| {
                DockArea::new(&mut dock)
                    .style(egui_dock::Style::from_egui(ui.style()))
                    .show_inside(ui, self);
            });
            self.dock_state = dock;
        }

        self.compile_window(&ctx);
        self.help_window(&ctx);
        self.diagnose_window(&ctx);
        self.import_window(&ctx);

        // Advance playback if we're currently playing.
        self.tick_playback();
        // Advance an in-progress interval-capture sequence.
        self.tick_interval();

        // Keep live view, capture spinner, compile progress, playback and interval countdowns repainting smoothly.
        if self.live.is_some()
            || self.capturing
            || self.compile.running.is_some()
            || self.mode.is_playing()
            || self.interval.is_some()
        {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
    }
}
