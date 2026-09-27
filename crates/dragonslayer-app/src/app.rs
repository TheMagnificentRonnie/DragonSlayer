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
    /// When the last live view frame arrived, so the diagnosis can tell if live view is flowing.
    last_live_at: Option<Instant>,
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
            last_live_at: None,
            renaming: None,
            drag_from: None,
            compile: CompileDialog::default(),
            message: None,
            keys: Vec::new(),
            _awake: None,
        };
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
        if self.capturing || !self.camera_ready() {
            return;
        }
        if let Some(p) = &self.project
            && self.session.cmd.send(Cmd::Capture(p.clone())).is_ok() {
                self.capturing = true;
            }
    }

    /// Kicks off an N-shot interval sequence. First shot fires immediately;
    /// subsequent shots fire `every` seconds after the previous one COMPLETES,
    /// so a slow camera slides the schedule instead of pileup.
    fn start_interval(&mut self) {
        if self.interval.is_some() || self.interval_count == 0 || !self.mode.is_capture() {
            return;
        }
        self.interval =
            Some(Interval { total: self.interval_count, done: 0, every: self.interval_secs, next_at: Some(Instant::now()) });
        self.tick_interval();
    }

    fn stop_interval(&mut self, reason: &str) {
        if let Some(iv) = self.interval.take() {
            self.info(format!("Interval stopped ({reason}): {}/{} frames captured", iv.done, iv.total));
        }
    }

    /// If an interval is running and the next scheduled time has arrived, fire
    /// the next capture. Called from `update()` every repaint and after each
    /// `Event::Captured`.
    fn tick_interval(&mut self) {
        let should_fire = match self.interval {
            Some(iv) if !self.capturing && iv.done < iv.total => {
                iv.next_at.is_some_and(|at| Instant::now() >= at)
            }
            _ => false,
        };
        if !should_fire {
            return;
        }
        if !self.mode.is_capture() {
            self.stop_interval("switched to Preview");
            return;
        }
        // Bail out cleanly if the camera or project disappeared under us.
        if !self.camera_ready() || self.project.is_none() {
            self.stop_interval("no camera");
            return;
        }
        if let Some(iv) = self.interval.as_mut() {
            iv.next_at = None;
        }
        self.capture();
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
                            if let Some(jpg) = c.files.iter().find(|f| dragonslayer_core::scene::is_jpeg(f)) {
                                self.images.prefetch(jpg, width);
                                self.images.prefetch(jpg, THUMB_WIDTH);
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
                                    self.info(format!("Interval: {done}/{total}"));
                                }
                            } else {
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

    fn shortcuts_active(&self) -> bool {
        self.renaming.is_none() && !self.compile.open && !self.help_open && !self.diagnose_open
    }

    /// Takes the shortcut keys out of the input before egui sees them, so a
    /// focused button can't swallow Space and Tab doesn't move keyboard focus.
    fn take_shortcuts(&mut self, raw: &mut egui::RawInput, typing: bool) {
        // While typing in a text or number field, Backspace and Space belong to the field,
        // not to delete-last-frame and capture.
        if !self.shortcuts_active() || typing {
            return;
        }
        // Keys we handle ourselves. Arrow keys and Home/End are navigation.
        const KEYS: [Key; 12] = [
            Key::Space, Key::Backspace, Key::O, Key::Tab, Key::H, Key::P, Key::F,
            Key::ArrowLeft, Key::ArrowRight, Key::Home, Key::End, Key::Escape,
        ];
        let mut pressed = Vec::new();
        // With Shift for coarser navigation.
        let mut pressed_shift = Vec::new();
        raw.events.retain(|e| match e {
            egui::Event::Key { key, pressed: down, repeat, modifiers, .. } if KEYS.contains(key) => {
                let plain = modifiers.is_none();
                let shift_only = modifiers.shift && !modifiers.ctrl && !modifiers.alt && !modifiers.command;
                if (plain || shift_only) && *down {
                    // Let arrow keys auto-repeat while held so scrubbing feels natural;
                    // everything else fires once per press.
                    let allow_repeat = matches!(key, Key::ArrowLeft | Key::ArrowRight);
                    if !*repeat || allow_repeat {
                        if shift_only {
                            pressed_shift.push(*key);
                        } else {
                            pressed.push(*key);
                        }
                    }
                    false
                } else {
                    true
                }
            }
            egui::Event::Text(t) => !matches!(t.as_str(), " " | "o" | "O" | "h" | "H" | "p" | "P" | "f" | "F"),
            _ => true,
        });
        self.keys.extend(pressed);
        // Shift+arrow → coarser step; we tag by pushing a sentinel None-encoding via a separate field
        // would need more scaffolding; simplest is to push the arrow multiple times.
        for k in pressed_shift {
            match k {
                Key::ArrowLeft | Key::ArrowRight => {
                    for _ in 0..10 { self.keys.push(k); }
                }
                _ => self.keys.push(k),
            }
        }
    }

    fn handle_keys(&mut self) {
        for key in std::mem::take(&mut self.keys) {
            // Minimal view is capture-only: no Preview navigation.
            if self.minimal
                && matches!(key, Key::Tab | Key::P | Key::ArrowLeft | Key::ArrowRight | Key::Home | Key::End)
            {
                continue;
            }
            match key {
                Key::Space => self.capture(),
                Key::Backspace => self.delete_last(),
                Key::O => self.onion_on = !self.onion_on,
                Key::Tab => self.toggle_live_last(),
                Key::H => self.help_open = !self.help_open,
                Key::P => self.toggle_play(),
                Key::ArrowLeft => self.step(-1),
                Key::ArrowRight => self.step(1),
                Key::Home => self.jump_to(Some(0)),
                Key::End => self.jump_to_end(),
                Key::F => self.toggle_minimal(),
                Key::Escape if self.minimal => self.minimal = false,
                Key::Escape => self.mode = Mode::Capture,
                _ => {}
            }
        }
    }

    /// Floating control bar for minimal view. Fades out when the mouse is still,
    /// stays while hovered.
    fn minimal_bar(&mut self, ctx: &egui::Context) {
        use egui_phosphor::regular as ph;
        let pal = crate::theme::palette();
        let idle = ctx.input(|i| i.pointer.time_since_last_movement());
        let visible = idle < 2.5 || self.minimal_bar_hovered || self.capturing;
        let opacity = ctx.animate_bool_with_time(egui::Id::new("minimal bar fade"), visible, 0.35);
        if opacity <= 0.0 {
            self.minimal_bar_hovered = false;
            return;
        }
        let area = egui::Area::new(egui::Id::new("minimal bar"))
            .anchor(Align2::CENTER_BOTTOM, Vec2::new(0.0, -24.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                ui.multiply_opacity(opacity);
                egui::Frame::popup(ui.style()).inner_margin(Margin::same(10)).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let (dot, tip) = match &self.status {
                            Status::Connected { name, .. } => (pal.ok, format!("{name} · connected")),
                            Status::Searching => (pal.warn, "No camera".to_string()),
                            Status::WrongDriver { .. } => (pal.warn, "Camera needs driver setup".to_string()),
                            Status::Problem { message, .. } => (pal.error, message.clone()),
                            Status::NoBackend => (Color32::GRAY, "No camera support in this build".to_string()),
                        };
                        let (r, resp) = ui.allocate_exact_size(Vec2::splat(12.0), Sense::hover());
                        ui.painter().circle_filled(r.center(), 5.0, dot);
                        resp.on_hover_text(tip);

                        if let Some(row) = self.active_row() {
                            ui.label(RichText::new(format!("{} · {} frames", row.name, row.count)).monospace());
                        }
                        ui.separator();

                        let can_capture = self.camera_ready() && !self.capturing && self.active_row().is_some();
                        let label = if self.capturing {
                            format!("{}  Capturing…", ph::RECORD)
                        } else {
                            format!("{}  Capture", ph::CAMERA)
                        };
                        let capture = egui::Button::new(RichText::new(label).size(16.0).strong().color(pal.on_accent))
                            .fill(pal.accent)
                            .min_size(Vec2::new(140.0, 36.0));
                        if ui.add_enabled(can_capture, capture).on_hover_text("Space").clicked() {
                            self.capture();
                        }
                        if let Some(iv) = self.interval {
                            ui.label(format!("{}  {}/{}", ph::TIMER, iv.done, iv.total));
                            if ui.button(format!("{}  Stop", ph::STOP)).clicked() {
                                self.stop_interval("stopped");
                            }
                        }
                        ui.separator();

                        if ui
                            .selectable_label(self.onion_on, format!("{}  Onion", ph::STACK))
                            .on_hover_text("O")
                            .clicked()
                        {
                            self.onion_on = !self.onion_on;
                        }
                        if ui.button(ph::QUESTION).on_hover_text("Help (H)").clicked() {
                            self.help_open = true;
                        }
                        if ui.button(format!("{}  Exit", ph::CORNERS_IN)).on_hover_text("F or Esc").clicked() {
                            self.minimal = false;
                        }
                    });
                });
            });
        self.minimal_bar_hovered = area.response.contains_pointer();
    }

    /// Minimal view needs a project to capture into, and always runs in Capture mode.
    fn toggle_minimal(&mut self) {
        if self.project.is_none() {
            return;
        }
        self.minimal = !self.minimal;
        if self.minimal {
            self.mode = Mode::Capture;
        }
    }

    // ---- navigator ------------------------------------------------------

    /// Ctrl-Tab-style toggle between live view and the last captured frame.
    fn toggle_live_last(&mut self) {
        self.mode = match self.mode {
            Mode::Capture => match self.frames.len().checked_sub(1) {
                Some(i) => Mode::Preview { index: i, playing: false, last_advance: Instant::now() },
                None => Mode::Capture,
            },
            _ => Mode::Capture,
        };
    }

    fn step(&mut self, delta: i32) {
        if self.frames.is_empty() {
            return;
        }
        let last = self.frames.len() - 1;
        let current = self.mode.preview_index().unwrap_or(last);
        let next = (current as i64 + delta as i64).clamp(0, last as i64) as usize;
        self.mode = Mode::Preview { index: next, playing: false, last_advance: Instant::now() };
    }

    fn jump_to(&mut self, idx: Option<usize>) {
        if self.frames.is_empty() {
            return;
        }
        let last = self.frames.len() - 1;
        let target = idx.unwrap_or(last).min(last);
        self.mode = Mode::Preview { index: target, playing: false, last_advance: Instant::now() };
    }

    fn jump_to_end(&mut self) {
        if self.frames.is_empty() {
            return;
        }
        self.mode = Mode::Preview { index: self.frames.len() - 1, playing: false, last_advance: Instant::now() };
    }

    fn toggle_play(&mut self) {
        if self.frames.is_empty() {
            return;
        }
        self.mode = match self.mode {
            Mode::Preview { index, playing: true, .. } => {
                // Pause on the current frame.
                Mode::Preview { index, playing: false, last_advance: Instant::now() }
            }
            Mode::Capture => Mode::Preview { index: 0, playing: true, last_advance: Instant::now() },
            Mode::Preview { index, playing: false, .. } => {
                let start = if index == self.frames.len() - 1 { 0 } else { index };
                Mode::Preview { index: start, playing: true, last_advance: Instant::now() }
            }
        };
        self.playback_tick = Instant::now();
    }

    /// Advance the playback cursor by one frame if the fps interval has passed.
    fn tick_playback(&mut self) {
        // Only advance while actually playing. Bug from an earlier refactor was matching
        // any Preview state here, which auto-advanced frames after every keypress.
        let Mode::Preview { index, last_advance, playing: true } = self.mode else { return };
        if self.frames.is_empty() {
            self.mode = Mode::Capture;
            return;
        }
        let fps = self
            .project
            .as_ref()
            .and_then(|p| p.active_scene().ok().map(|s| p.fps_for(&s)))
            .unwrap_or(12)
            .max(1);
        let step = Duration::from_secs_f64(1.0 / f64::from(fps));
        let elapsed = last_advance.elapsed();
        if elapsed < step {
            return;
        }
        // Handle "we fell behind by multiple frames" gracefully.
        let steps = (elapsed.as_secs_f64() / step.as_secs_f64()).floor() as usize;
        let next = index + steps;
        if next >= self.frames.len() {
            // End of scene → pause on last frame.
            self.mode = Mode::Preview { index: self.frames.len() - 1, playing: false, last_advance: Instant::now() };
        } else {
            self.mode = Mode::Preview { index: next, playing: true, last_advance: last_advance + step * (steps as u32),
             };
        }
        self.playback_tick = Instant::now();
    }

    // ---- ui -------------------------------------------------------------

    /// Classic app menu bar: File / Edit / View / Scene / Compile / Help.
    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        use egui_phosphor::regular as ph;
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button(format!("{}  File", ph::FILE), |ui| {
                if ui.button(format!("{}  New project…", ph::FILE_PLUS)).clicked() {
                    self.new_project_dialog();
                    ui.close();
                }
                if ui.button(format!("{}  Open project…", ph::FOLDER_OPEN)).clicked() {
                    if let Some(dir) = rfd::FileDialog::new()
                        .set_title("Open a DragonSlayer project folder")
                        .pick_folder()
                    {
                        self.open_project(&dir);
                    }
                    ui.close();
                }
                if let Some(p) = &self.project {
                    ui.separator();
                    if ui.button(format!("{}  Reveal project folder", ph::FOLDER_OPEN)).clicked() {
                        reveal(&p.root);
                        ui.close();
                    }
                }
                ui.separator();
                ui.menu_button(format!("{}  Preferences", ph::GEAR), |ui| {
                    ui.label(RichText::new("Theme").small().color(crate::theme::palette().text_muted));
                    let ctx = ui.ctx().clone();
                    for choice in crate::theme::ThemeChoice::ALL {
                        if ui.radio(self.theme_choice == choice, choice.label()).clicked() {
                            self.theme_choice = choice;
                            crate::theme::install(&ctx, choice);
                        }
                    }
                    ui.separator();
                    ui.label(RichText::new("Viewer").small().color(crate::theme::palette().text_muted));
                    ui.checkbox(&mut self.fill_viewer, format!("{}  Fill viewer (crop edges)", ph::CROP))
                        .on_hover_text("Scale the picture to fill the viewer. Turn off to see the whole frame with bars.");
                    ui.checkbox(&mut self.pip_on, format!("{}  Picture-in-picture", ph::IMAGE_SQUARE))
                        .on_hover_text("Shows the last captured frame in a corner while on live view, or the live feed while in Preview.");
                });
                ui.separator();
                if ui.button(format!("{}  Quit", ph::SIGN_OUT)).clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            ui.menu_button(format!("{}  View", ph::EYE), |ui| {
                if ui
                    .selectable_label(self.mode.is_capture(), format!("{}  Capture", ph::VIDEO_CAMERA))
                    .clicked()
                {
                    self.mode = Mode::Capture;
                    ui.close();
                }
                let can_preview = !self.frames.is_empty();
                let preview_selected = !self.mode.is_capture();
                let preview_label = format!("{}  Preview", ph::FILM_STRIP);
                let clicked = ui
                    .add_enabled_ui(can_preview, |ui| ui.selectable_label(preview_selected, preview_label))
                    .inner
                    .clicked();
                if clicked {
                    self.jump_to_end();
                    ui.close();
                }
                if ui
                    .add_enabled(self.project.is_some(), egui::Button::new(format!("{}  Minimal view (F)", ph::CORNERS_OUT)))
                    .on_hover_text("Full-screen live view with a small floating control bar")
                    .clicked()
                {
                    self.toggle_minimal();
                    ui.close();
                }
                ui.separator();
                ui.checkbox(&mut self.onion_on, format!("{}  Onion skin (O)", ph::STACK));
                ui.checkbox(&mut self.onion_edges, format!("{}  Outlines only", ph::EYE));
            });
            ui.menu_button(format!("{}  Scene", ph::FILM_STRIP), |ui| {
                let can = self.project.is_some();
                if ui.add_enabled(can, egui::Button::new(format!("{}  Add scene", ph::PLUS))).clicked() {
                    let n = self.scenes.len() + 1;
                    let after = self.active_row().map(|r| r.id.clone());
                    self.edit(|p| p.add_scene(&format!("Scene {n}"), after.as_deref()).map(|_| ()));
                    ui.close();
                }
                if ui.add_enabled(self.active_row().is_some(), egui::Button::new(format!("{}  Rename active scene", ph::PENCIL_SIMPLE))).clicked() {
                    if let Some(row) = self.active_row() {
                        self.renaming = Some((row.id.clone(), row.name.clone()));
                    }
                    ui.close();
                }
                if ui
                    .add_enabled(
                        self.active_row().is_some_and(|r| r.count > 0),
                        egui::Button::new(format!("{}  Delete last frame", ph::TRASH)),
                    )
                    .clicked()
                {
                    self.delete_last();
                    ui.close();
                }
            });
            ui.menu_button(format!("{}  Compile", ph::EXPORT), |ui| {
                if ui
                    .add_enabled(self.project.is_some(), egui::Button::new(format!("{}  Compile…", ph::EXPORT)))
                    .clicked()
                {
                    self.compile.open = true;
                    self.compile.result = None;
                    ui.close();
                }
            });
            ui.menu_button(format!("{}  Help", ph::QUESTION), |ui| {
                if ui.button(format!("{}  In-app help (H)", ph::QUESTION)).clicked() {
                    self.help_open = true;
                    ui.close();
                }
                if ui.button(format!("{}  Diagnose camera…", ph::STETHOSCOPE)).clicked() {
                    self.diagnose_open = true;
                    self.start_usb_scan(ui.ctx());
                    ui.close();
                }
            });

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(
                    RichText::new(concat!("v", env!("CARGO_PKG_VERSION")))
                        .small()
                        .color(crate::theme::palette().text_dim),
                );
                ui.label(RichText::new("DragonSlayer").strong().color(crate::theme::palette().accent));
            });
        });
    }

    /// Second row: project breadcrumb + camera status + fps picker + mode toggle.
    fn status_bar(&mut self, ui: &mut egui::Ui) {
        use egui_phosphor::regular as ph;
        ui.horizontal(|ui| {
            if let Some(p) = &self.project {
                ui.label(RichText::new(format!("{}  {}", ph::FOLDER_OPEN, p.name())).strong());
                ui.separator();
                let mut fps = p.file.fps;
                egui::ComboBox::from_id_salt("project fps")
                    .selected_text(format!("{fps} fps"))
                    .show_ui(ui, |ui| {
                        for choice in [6, 8, 10, 12, 15, 18, 24, 25, 30] {
                            ui.selectable_value(&mut fps, choice, format!("{choice} fps"));
                        }
                    });
                if fps != p.file.fps {
                    self.edit(|p| {
                        p.file.fps = fps;
                        p.save()
                    });
                }
                ui.separator();
                // Mode chip — segmented, icon-driven.
                let has_frames = !self.frames.is_empty();
                let in_add = self.mode.is_capture();
                let add_txt = format!("{}  Capture", ph::VIDEO_CAMERA);
                if ui.selectable_label(in_add, add_txt).clicked() {
                    self.mode = Mode::Capture;
                }
                let prev_txt = format!("{}  Preview", ph::FILM_STRIP);
                let clicked = ui
                    .add_enabled_ui(has_frames, |ui| ui.selectable_label(!in_add, prev_txt))
                    .inner
                    .clicked();
                if clicked {
                    self.jump_to_end();
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button(format!("{}  Help", ph::QUESTION)).on_hover_text("H").clicked() {
                    self.help_open = true;
                }
                if self.project.is_some()
                    && ui.button(ph::CORNERS_OUT).on_hover_text("Minimal view (F): full-screen capture").clicked()
                {
                    self.toggle_minimal();
                }
                ui.separator();
                self.camera_status(ui);
            });
        });
    }

    // ---- dockable tab bodies -------------------------------------------

    fn tab_scenes(&mut self, ui: &mut egui::Ui) {
        use egui_phosphor::regular as ph;
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.button(format!("{}  Add scene", ph::PLUS)).clicked() {
                let n = self.scenes.len() + 1;
                let after = self.active_row().map(|r| r.id.clone());
                self.edit(|p| p.add_scene(&format!("Scene {n}"), after.as_deref()).map(|_| ()));
            }
        });
        ui.separator();
        self.scene_list(ui);
    }

    fn tab_camera(&mut self, ui: &mut egui::Ui) {
        use egui_phosphor::regular as ph;
        ui.add_space(4.0);
        egui::Frame::new()
            .inner_margin(Margin::same(4))
            .show(ui, |ui| self.camera_status(ui));
        ui.add_space(6.0);
        let in_capture = self.mode.is_capture();
        let can_capture = in_capture && self.camera_ready() && !self.capturing && self.active_row().is_some();
        let label = if self.capturing {
            format!("{}  Capturing…", ph::RECORD)
        } else {
            format!("{}  Capture", ph::CAMERA)
        };
        let capture = egui::Button::new(RichText::new(label).size(16.0).strong().color(crate::theme::palette().on_accent))
            .fill(crate::theme::palette().accent)
            .min_size(Vec2::new(ui.available_width(), 44.0));
        let resp = ui.add_enabled(can_capture, capture).on_hover_text("Space");
        let resp = if in_capture { resp } else { resp.on_disabled_hover_text("Preview mode: switch to Capture (Tab) to take frames") };
        if resp.clicked() {
            self.capture();
        }
        let has_frames = self.active_row().is_some_and(|r| r.count > 0);
        if ui
            .add_enabled(
                has_frames,
                egui::Button::new(format!("{}  Delete last frame", ph::TRASH))
                    .min_size(Vec2::new(ui.available_width(), 32.0)),
            )
            .on_hover_text("Backspace — moves the frame to the scene's trash")
            .clicked()
        {
            self.delete_last();
        }
        ui.add_space(10.0);
        self.interval_ui(ui);
        ui.add_space(10.0);
        ui.separator();
        if ui
            .button(format!("{}  Diagnose camera…", ph::STETHOSCOPE))
            .on_hover_text("Checks the USB connection, the Windows driver, and whether the camera answers")
            .clicked()
        {
            self.diagnose_open = true;
            self.start_usb_scan(ui.ctx());
        }
    }

    /// Interval-capture controls: N frames, every S seconds. Start/Stop toggles.
    fn interval_ui(&mut self, ui: &mut egui::Ui) {
        use egui_phosphor::regular as ph;
        ui.separator();
        ui.label(RichText::new("Interval capture").strong());
        ui.label(
            RichText::new("Fire N shots automatically, S seconds apart. Great for time-lapses of a set-up or a puppet slumping.")
                .small()
                .color(crate::theme::palette().text_muted),
        );
        ui.add_space(4.0);

        let running = self.interval.is_some();
        ui.add_enabled_ui(!running, |ui| {
            ui.horizontal(|ui| {
                ui.label("Frames");
                ui.add(egui::DragValue::new(&mut self.interval_count).range(1..=9_999).speed(1.0));
                ui.label("every");
                ui.add(
                    egui::DragValue::new(&mut self.interval_secs)
                        .range(0.0..=3600.0)
                        .speed(0.1)
                        .suffix(" s"),
                );
            });
        });

        ui.add_space(4.0);

        match self.interval {
            None => {
                let can_start = self.mode.is_capture() && self.camera_ready() && self.active_row().is_some();
                if ui
                    .add_enabled(
                        can_start,
                        egui::Button::new(format!("{}  Start interval", ph::PLAY))
                            .min_size(Vec2::new(ui.available_width(), 32.0)),
                    )
                    .clicked()
                {
                    self.start_interval();
                }
            }
            Some(iv) => {
                let remaining = iv.next_at.map(|at| at.saturating_duration_since(Instant::now())).unwrap_or_default();
                let status = if self.capturing {
                    format!("Capturing {}/{}…", iv.done + 1, iv.total)
                } else {
                    format!("Next in {:.1}s  ({}/{})", remaining.as_secs_f64(), iv.done, iv.total)
                };
                ui.label(RichText::new(status).color(crate::theme::palette().accent));
                if ui
                    .add(
                        egui::Button::new(format!("{}  Stop", ph::STOP))
                            .min_size(Vec2::new(ui.available_width(), 32.0)),
                    )
                    .clicked()
                {
                    self.stop_interval("stopped by user");
                }
            }
        }
    }

    fn tab_exposure(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        let muted = crate::theme::palette().text_muted;
        if !self.camera_ready() {
            ui.label(RichText::new("Connect a camera to change its settings.").color(muted));
            return;
        }
        if self.camera_settings.is_empty() {
            ui.label(RichText::new("This camera doesn't report any adjustable settings over USB.").color(muted));
            return;
        }

        let mut change: Option<(dragonslayer_camera::SettingKind, String)> = None;
        // Changing settings mid-capture would race the shot.
        let enabled = !self.capturing && !self.setting_pending;
        egui::Grid::new("exposure").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
            for s in &self.camera_settings {
                ui.label(s.kind.label());
                let mut selected = s.value.clone();
                ui.add_enabled_ui(enabled && !s.readonly, |ui| {
                    egui::ComboBox::from_id_salt(s.kind)
                        .selected_text(&s.value)
                        .width(ui.available_width().max(140.0))
                        .show_ui(ui, |ui| {
                            for c in &s.choices {
                                ui.selectable_value(&mut selected, c.clone(), c);
                            }
                        })
                        .response
                        .on_disabled_hover_text(if s.readonly {
                            "Read-only in the camera's current mode. Set the mode dial to M."
                        } else {
                            "Waiting for the camera…"
                        });
                });
                if selected != s.value {
                    change = Some((s.kind, selected));
                }
                ui.end_row();
            }
        });
        if let Some((kind, value)) = change
            && self.session.cmd.send(Cmd::SetSetting(kind, value)).is_ok()
        {
            self.setting_pending = true;
        }

        ui.add_space(8.0);
        ui.label(
            RichText::new(
                "Shoot in manual (M) with a fixed white balance: anything automatic changes between frames and makes the film flicker.",
            )
            .small()
            .color(muted),
        );
    }

    fn tab_onion(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        ui.checkbox(&mut self.onion_on, "Show onion skin (O)");
        ui.add_enabled_ui(self.onion_on, |ui| {
            ui.add(egui::Slider::new(&mut self.onion_count, 1..=5).text("frames"));
            ui.add(egui::Slider::new(&mut self.onion_opacity, 0.05..=0.9).text("opacity"));
            ui.checkbox(&mut self.onion_edges, "Outlines only (crisper over live view)");
        });
    }

    fn tab_export(&mut self, ui: &mut egui::Ui) {
        use egui_phosphor::regular as ph;
        ui.add_space(4.0);
        if ui
            .add(
                egui::Button::new(format!("{}  Compile video…", ph::EXPORT))
                    .min_size(Vec2::new(ui.available_width(), 36.0)),
            )
            .clicked()
        {
            self.compile.open = true;
            self.compile.result = None;
        }
        ui.add_space(6.0);
        ui.label(
            RichText::new("Renders your scenes to MP4 or ProRes MOV using ffmpeg. Files land in exports/ inside the project folder and are never overwritten.")
                .small()
                .color(crate::theme::palette().text_muted),
        );
    }

    fn tab_timeline(&mut self, ui: &mut egui::Ui) {
        use egui_phosphor::regular as ph;
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            let has_frames = !self.frames.is_empty();
            let playing = self.mode.is_playing();
            ui.add_enabled_ui(has_frames, |ui| {
                if ui.button(ph::SKIP_BACK).on_hover_text("Home — first frame").clicked() {
                    self.jump_to(Some(0));
                }
                if ui.button(ph::CARET_LEFT).on_hover_text("← previous frame (Shift ×10)").clicked() {
                    self.step(-1);
                }
                let play_icon = if playing { ph::PAUSE } else { ph::PLAY };
                let play_btn = egui::Button::new(RichText::new(play_icon).size(18.0).strong().color(crate::theme::palette().on_accent))
                    .fill(crate::theme::palette().accent)
                    .min_size(Vec2::new(52.0, 30.0));
                if ui.add(play_btn).on_hover_text("P — play/pause at scene fps").clicked() {
                    self.toggle_play();
                }
                if ui.button(ph::CARET_RIGHT).on_hover_text("→ next frame (Shift ×10)").clicked() {
                    self.step(1);
                }
                if ui.button(ph::SKIP_FORWARD).on_hover_text("End — last frame").clicked() {
                    self.jump_to_end();
                }
            });
            if !self.frames.is_empty() {
                let n = self.frames.len();
                let idx = self.mode.preview_index().unwrap_or(n - 1);
                ui.separator();
                ui.label(
                    RichText::new(format!("Frame {} / {}", idx + 1, n))
                        .monospace()
                        .color(crate::theme::palette().text_muted),
                );
            }
        });
        self.timeline(ui);
    }

    /// Checks USB devices in the background (a PowerShell query, ~2 s). Windows only.
    fn start_usb_scan(&mut self, ctx: &egui::Context) {
        if !cfg!(windows) || self.usb_scan_rx.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        thread::spawn(move || {
            let _ = tx.send(dragonslayer_camera::diag::scan());
            ctx.request_repaint();
        });
        self.usb_scan_rx = Some(rx);
        self.usb_scan_at = Some(Instant::now());
    }

    /// While no camera is connected, re-check USB every 10 s so a camera stuck on
    /// Windows' own driver (wrong port for Zadig) is pointed out without asking.
    fn auto_usb_scan(&mut self, ctx: &egui::Context) {
        let looking = matches!(self.status, Status::Searching | Status::Problem { .. } | Status::WrongDriver { .. });
        if looking && self.usb_scan_at.is_none_or(|t| t.elapsed() >= Duration::from_secs(10)) {
            self.start_usb_scan(ctx);
        }
    }

    /// A plugged-in camera on Windows' own driver, according to the last USB check.
    fn camera_on_windows_driver(&self) -> Option<&UsbCamera> {
        self.usb_scan.as_ref()?.as_ref().ok()?.iter().find(|c| !c.driver.usable())
    }

    fn camera_status(&mut self, ui: &mut egui::Ui) {
        // Actionable driver state gets its own row with a Set up button. The USB check
        // catches it even when the camera library doesn't list the camera at all.
        let needs_driver = match &self.status {
            Status::WrongDriver { name } => Some(name.clone().unwrap_or_else(|| "Camera".into())),
            Status::Searching | Status::Problem { .. } => self.camera_on_windows_driver().map(|c| c.name.clone()),
            _ => None,
        };
        if let Some(n) = needs_driver {
            if ui
                .small_button(egui_phosphor::regular::STETHOSCOPE)
                .on_hover_text("Diagnose camera")
                .clicked()
            {
                self.diagnose_open = true;
                self.start_usb_scan(ui.ctx());
            }
            if ui
                .button("Set up USB driver…")
                .on_hover_text("One-time driver swap so DragonSlayer can talk to the camera")
                .clicked()
            {
                launch_driver_setup();
            }
            ui.label(RichText::new(format!("{n} · this USB port needs driver setup")).color(crate::theme::palette().warn));
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
            ui.painter().circle_filled(rect.center(), 5.0, crate::theme::palette().warn);
            return;
        }
        let (dot, text, detail) = match &self.status {
            Status::Connected { name, caps } => (
                crate::theme::palette().ok,
                format!("{name} · connected"),
                (!caps.live_view).then(|| "No live view on this model: onion skin shows over the last frame.".to_string()),
            ),
            Status::Searching => (
                crate::theme::palette().warn,
                "No camera".into(),
                Some(connect_hint().to_string()),
            ),
            Status::NoBackend => (
                Color32::GRAY,
                "No camera support in this build".into(),
                Some("Rebuild with `--features gphoto2`, or start with --mock to try the mock camera.".into()),
            ),
            Status::Problem { name, message } => (
                crate::theme::palette().error,
                name.clone().unwrap_or_else(|| "Camera".into()) + " · problem",
                Some(message.clone()),
            ),
            Status::WrongDriver { .. } => unreachable!(),
        };
        if matches!(self.status, Status::Searching | Status::Problem { .. })
            && ui
                .small_button(egui_phosphor::regular::STETHOSCOPE)
                .on_hover_text("Diagnose camera")
                .clicked()
        {
            self.diagnose_open = true;
            self.start_usb_scan(ui.ctx());
        }
        if let Some(detail) = &detail {
            ui.label(RichText::new(detail).small().color(ui.visuals().weak_text_color())).on_hover_text(detail);
        }
        ui.label(text);
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
        ui.painter().circle_filled(rect.center(), 5.0, dot);
    }

    fn new_project_dialog(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .set_title("New project: choose a folder name")
            .set_file_name("My Film")
            .save_file()
        else {
            return;
        };
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "My Film".into());
        match Project::create(&path, &name, dragonslayer_core::project::DEFAULT_FPS) {
            Ok(p) => {
                self.info(format!("Created {name}"));
                self.set_project(p);
            }
            Err(e) => self.error(e),
        }
    }

    fn welcome(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.3);
            ui.heading("Make a stop-motion film with your camera");
            ui.add_space(12.0);
            if ui.add(egui::Button::new(RichText::new("New project").size(18.0))).clicked() {
                self.new_project_dialog();
            }
            ui.add_space(6.0);
            if ui.button("Open a project folder…").clicked()
                && let Some(dir) = rfd::FileDialog::new().pick_folder() {
                    self.open_project(&dir);
                }
        });
    }

    fn scene_list(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        ui.label(RichText::new("Scenes").strong());
        ui.add_space(4.0);

        let active = self.project.as_ref().and_then(|p| p.file.active_scene.clone());
        let mut rects = Vec::with_capacity(self.scenes.len());
        let mut action: Option<SceneAction> = None;

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for (i, row) in self.scenes.iter().enumerate() {
                let is_active = active.as_deref() == Some(row.id.as_str());
                let resp = scene_row(ui, row, is_active, &mut self.images, &mut self.renaming);
                rects.push(resp.rect);
                if resp.clicked() {
                    action = Some(SceneAction::Activate(row.id.clone()));
                }
                if resp.double_clicked() {
                    self.renaming = Some((row.id.clone(), row.name.clone()));
                }
                if resp.drag_started() {
                    self.drag_from = Some(i);
                }
                resp.context_menu(|ui| {
                    if ui.button("Rename").clicked() {
                        self.renaming = Some((row.id.clone(), row.name.clone()));
                        ui.close();
                    }
                    ui.menu_button("Frame rate", |ui| {
                        if ui.radio(row.fps.is_none(), "Same as project").clicked() {
                            action = Some(SceneAction::Fps(row.id.clone(), None));
                        }
                        for fps in [6, 8, 10, 12, 15, 24, 25, 30] {
                            if ui.radio(row.fps == Some(fps), format!("{fps} fps")).clicked() {
                                action = Some(SceneAction::Fps(row.id.clone(), Some(fps)));
                            }
                        }
                    });
                    ui.separator();
                    if ui.button("Delete scene (moves to trash)").clicked() {
                        action = Some(SceneAction::Delete(row.id.clone()));
                        ui.close();
                    }
                });
            }

            ui.add_space(6.0);
            if ui.button("+ Add scene").clicked() {
                action = Some(SceneAction::Add);
            }
        });

        // Drag to reorder: draw a drop marker and move on release.
        if let Some(from) = self.drag_from {
            let pointer = ui.ctx().pointer_interact_pos();
            let target = pointer.map(|p| {
                rects.iter().position(|r| p.y < r.center().y).unwrap_or(rects.len())
            });
            if let (Some(t), Some(first)) = (target, rects.first()) {
                let y = rects.get(t).map_or_else(|| rects.last().unwrap().bottom(), |r| r.top());
                ui.painter().hline(first.x_range(), y, Stroke::new(2.0, ui.visuals().selection.bg_fill));
            }
            if ui.input(|i| i.pointer.any_released()) {
                self.drag_from = None;
                if let Some(t) = target {
                    let to = if t > from { t - 1 } else { t };
                    if to != from {
                        action = Some(SceneAction::Move(self.scenes[from].id.clone(), to));
                    }
                }
            }
        }

        if let Some((id, name)) = self.renaming.clone() {
            if ui.input(|i| i.key_pressed(Key::Enter)) {
                self.renaming = None;
                let name = name.trim().to_string();
                if !name.is_empty() {
                    action = Some(SceneAction::Rename(id, name));
                }
            } else if ui.input(|i| i.key_pressed(Key::Escape)) {
                self.renaming = None;
            }
        }

        match action {
            Some(SceneAction::Activate(id)) => self.edit(|p| p.set_active(&id)),
            Some(SceneAction::Add) => {
                let n = self.scenes.len() + 1;
                let after = active.clone();
                self.edit(|p| p.add_scene(&format!("Scene {n}"), after.as_deref()).map(|_| ()));
            }
            Some(SceneAction::Rename(id, name)) => self.edit(|p| p.rename_scene(&id, &name)),
            Some(SceneAction::Fps(id, fps)) => self.edit(|p| p.set_scene_fps(&id, fps)),
            Some(SceneAction::Move(id, to)) => self.edit(|p| p.move_scene(&id, to)),
            Some(SceneAction::Delete(id)) => {
                self.edit(|p| p.delete_scene(&id));
                self.info("Scene moved to the project's trash folder");
            }
            None => {}
        }
    }

    fn viewer(&mut self, ui: &mut egui::Ui) {
        let area = ui.available_rect_before_wrap();
        ui.painter().rect_filled(area, 0.0, Color32::from_gray(18));

        let width = self.onion_width();
        let n = self.frames.len();

        // Base image and where onion frames should end (exclusive) depend on viewer state.
        let (base, onion_end, mode_label) = match self.mode {
            Mode::Capture => {
                let live = self.has_live_view().then(|| self.live.clone()).flatten();
                match live {
                    Some(t) => (Some(t), n, "LIVE".to_string()),
                    None => {
                        // No live view — fall back to the last captured frame as background.
                        let last = self.frames.last().and_then(|f| f.jpeg()).map(Path::to_path_buf);
                        (last.and_then(|p| self.images.get(&p, width)), n.saturating_sub(1), "LIVE (no feed)".to_string())
                    }
                }
            }
            Mode::Preview { index, playing, .. } => {
                let base = self.frames.get(index).and_then(|f| f.jpeg()).and_then(|p| self.images.get(p, width));
                let label = if playing {
                    format!("PLAY {}/{}", index + 1, n.max(1))
                } else {
                    format!("PREVIEW {}/{}", index + 1, n.max(1))
                };
                (base, index, label)
            }
        };
        let showing_live = self.mode.is_capture() && self.live.is_some();

        // Onion frames: the N frames before the base, oldest first.
        let mut onions = Vec::new();
        if self.onion_on {
            let kind = if self.onion_edges { crate::images::Kind::Edges } else { crate::images::Kind::Full };
            let start = onion_end.saturating_sub(self.onion_count);
            for f in &self.frames[start..onion_end] {
                if let Some(p) = f.jpeg() {
                    onions.push(self.images.get_kind(p, width, kind));
                }
            }
        }

        let row = self.active_row();
        let scene_label = row.map(|r| format!("{} · {} frames", r.name, r.count)).unwrap_or_default();

        let Some(base) = base else {
            let text = if self.frames.is_empty() {
                match &self.status {
                    Status::Connected { .. } => "Waiting for live view…".to_string(),
                    _ => "Connect a camera to see live view".to_string(),
                }
            } else {
                "Loading…".to_string()
            };
            ui.painter().text(area.center(), Align2::CENTER_CENTER, text, FontId::proportional(18.0), Color32::GRAY);
            overlay(ui, area, &scene_label, "");
            return;
        };

        let (rect, uv) = if self.fill_viewer {
            (area, cover_uv(area, base.size_vec2()))
        } else {
            (fit(area.shrink(8.0), base.size_vec2()), Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)))
        };
        let painter = ui.painter_at(area);
        painter.image(base.id(), rect, uv, Color32::WHITE);
        // Tint onion frames cyan so the ghost separates visually from the live view.
        // Multiply the tint's RGBA by its own alpha for egui's premultiplied blend.
        let count = onions.len();
        for (i, tex) in onions.into_iter().enumerate() {
            let Some(tex) = tex else { continue };
            // Nearest frame strongest; older frames fade off faster than linear.
            let weight = ((i + 1) as f32 / count as f32).powf(1.4);
            let a = self.onion_opacity * weight;
            let (r, g, b) = (0.55 * a, 0.85 * a, 1.00 * a);
            let to_u8 = |x: f32| (x * 255.0).round().clamp(0.0, 255.0) as u8;
            let tint = Color32::from_rgba_premultiplied(to_u8(r), to_u8(g), to_u8(b), to_u8(a));
            painter.image(tex.id(), rect, uv, tint);
        }

        let _ = showing_live;
        overlay(ui, rect, &scene_label, &mode_label);

        // Picture-in-picture: always show the "other" view in the top-right corner —
        // unless that would duplicate what the main viewer is already showing
        // (e.g. no live feed → main falls back to last frame → PIP would repeat it).
        if !self.pip_on {
            return;
        }
        let main_is_live_feed = self.mode.is_capture() && self.live.is_some();
        let main_frame_idx: Option<usize> = if !self.mode.is_capture() {
            self.mode.preview_index()
        } else if self.live.is_none() {
            // Main fell back to last frame.
            self.frames.len().checked_sub(1)
        } else {
            None
        };

        let (pip_tex, pip_label, pip_dot, pip_border, swap_target, want_pip) =
            if self.mode.is_capture() {
                // Live main → last frame in the PIP (but only if main is a real live
                // feed; otherwise main is already showing the last frame).
                let last_idx = self.frames.len().checked_sub(1);
                let show = main_is_live_feed && last_idx.is_some();
                let tex = last_idx
                    .and_then(|i| self.frames.get(i).and_then(|f| f.jpeg()).and_then(|p| self.images.get(p, 320)));
                let label = last_idx.map(|i| format!("FRAME {}", i + 1)).unwrap_or_else(|| "NO FRAMES YET".into());
                (
                    tex,
                    label,
                    Color32::from_rgb(80, 170, 250),
                    Color32::from_rgb(80, 170, 250),
                    last_idx.map(|i| Mode::Preview { index: i, playing: false, last_advance: Instant::now() }),
                    show,
                )
            } else {
                // Frame/Playing main → live view in the PIP. Skip if the live texture
                // is missing AND the main isn't a frame we'd swap FROM (i.e. no camera).
                let show = self.live.is_some() && Some(main_frame_idx.unwrap_or(usize::MAX)) != Some(usize::MAX);
                (
                    self.live.clone(),
                    "LIVE".to_string(),
                    crate::theme::palette().live,
                    crate::theme::palette().live,
                    Some(Mode::Capture),
                    show,
                )
            };
        let placeholder = want_pip;

        // Always draw PIP if there's something meaningful to show (a frame exists or
        // live is available). Placeholder rendered while texture loads.
        if placeholder {
            let pip_w = (area.width() * 0.22).clamp(180.0, 320.0);
            let pip_h = pip_w * 0.5625; // 16:9 placeholder aspect
            let (pip_h, aspect_source) = match &pip_tex {
                Some(t) => {
                    let size = t.size_vec2();
                    let a = if size.x > 0.0 { size.y / size.x } else { 0.5625 };
                    (pip_w * a, Some(a))
                }
                None => (pip_h, None),
            };
            let _ = aspect_source;
            let margin = 12.0;
            let pip_rect = Rect::from_min_size(
                egui::pos2(rect.right() - pip_w - margin, rect.top() + margin),
                Vec2::new(pip_w, pip_h),
            );
            painter.rect_filled(pip_rect.expand(4.0), 4.0, Color32::from_black_alpha(200));
            painter.rect_filled(pip_rect, 3.0, Color32::from_gray(30));
            if let Some(tex) = &pip_tex {
                painter.image(
                    tex.id(),
                    pip_rect,
                    Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            } else {
                painter.text(
                    pip_rect.center(),
                    Align2::CENTER_CENTER,
                    "loading…",
                    FontId::proportional(14.0),
                    Color32::from_gray(160),
                );
            }
            painter.rect_stroke(pip_rect, 3.0, Stroke::new(1.5, pip_border), egui::epaint::StrokeKind::Outside);
            let dot = pip_rect.left_top() + Vec2::new(10.0, 10.0);
            painter.circle_filled(dot, 4.0, pip_dot);
            painter.text(
                dot + Vec2::new(10.0, -2.0),
                Align2::LEFT_TOP,
                &pip_label,
                FontId::proportional(12.0),
                Color32::WHITE,
            );
            let _ = swap_target;
        }
    }

    /// Filmstrip timeline: clickable thumbnails of the active scene's frames
    /// with a highlighted playhead. Painted just above the controls bar.
    fn timeline(&mut self, ui: &mut egui::Ui) {
        let n = self.frames.len();
        if n == 0 {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("No frames yet — press Space to capture the first one").small().color(ui.visuals().weak_text_color()));
            });
            ui.add_space(4.0);
            return;
        }
        let height = 62.0;
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 3.0, Color32::from_gray(24));

        let thumb_w = 74.0;
        let thumb_h = height - 10.0;
        let gap = 4.0;
        let stride = thumb_w + gap;
        let visible = ((rect.width() - 8.0) / stride).floor().max(1.0) as usize;

        // Centre the playhead: show a window of `visible` frames centred on the current index when possible.
        let cur = self.mode.preview_index().unwrap_or(n.saturating_sub(1));
        let half = visible / 2;
        let start = cur.saturating_sub(half).min(n.saturating_sub(visible.min(n)));
        let end = (start + visible).min(n);

        for (draw_i, i) in (start..end).enumerate() {
            let x = rect.left() + 4.0 + draw_i as f32 * stride;
            let r = Rect::from_min_size(egui::pos2(x, rect.top() + 5.0), Vec2::new(thumb_w, thumb_h));
            let is_current = i == cur && !self.mode.is_capture();
            painter.rect_filled(r, 2.0, Color32::from_gray(35));
            if let Some(tex) = self.frames[i].jpeg().and_then(|p| self.images.get(p, 160)) {
                let inner = fit(r.shrink(2.0), tex.size_vec2());
                painter.image(tex.id(), inner, Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
            }
            if is_current {
                painter.rect_stroke(r, 2.0, Stroke::new(2.0, Color32::from_rgb(80, 170, 250)), egui::epaint::StrokeKind::Outside);
            }
            painter.text(
                egui::pos2(r.center().x, r.bottom() - 4.0),
                Align2::CENTER_BOTTOM,
                &self.frames[i].id,
                FontId::proportional(10.0),
                Color32::from_gray(170),
            );
        }

        // Click to jump to a frame.
        if (resp.clicked() || resp.dragged())
            && let Some(p) = resp.interact_pointer_pos()
        {
            let x = p.x - rect.left() - 4.0;
            if x >= 0.0 {
                let draw_i = (x / stride).floor() as usize;
                let target = (start + draw_i).min(n - 1);
                if self.mode.is_capture() {
                    log_line(&format!("timeline {} at frame {}", if resp.clicked() { "click" } else { "drag" }, target + 1));
                }
                self.mode = Mode::Preview { index: target, playing: false, last_advance: Instant::now() };
            }
        }
    }

    fn compile_window(&mut self, ctx: &egui::Context) {
        if !self.compile.open {
            return;
        }
        let mut open = true;
        let active_name = self.active_row().map(|r| r.name.clone());
        egui::Window::new("Compile video")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                let d = &mut self.compile;
                let busy = d.running.is_some();
                ui.add_enabled_ui(!busy, |ui| {
                    egui::Grid::new("compile").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                        ui.label("What");
                        ui.horizontal(|ui| {
                            ui.radio_value(&mut d.scope, Scope::Project, "Whole project");
                            if let Some(name) = &active_name {
                                ui.radio_value(&mut d.scope, Scope::Scene, format!("This scene ({name})"));
                            }
                        });
                        ui.end_row();

                        ui.label("Format");
                        ui.horizontal(|ui| {
                            ui.radio_value(&mut d.format, Format::H264, "MP4 (H.264, plays everywhere)");
                            ui.radio_value(&mut d.format, Format::ProRes, "MOV (ProRes 422, for editing)");
                        });
                        ui.end_row();

                        ui.label("Size");
                        ui.horizontal(|ui| {
                            ui.radio_value(&mut d.resolution, Resolution::Source, "Same as photos");
                            ui.radio_value(&mut d.resolution, Resolution::Uhd, "4K");
                            ui.radio_value(&mut d.resolution, Resolution::Hd, "1080p");
                        });
                        ui.end_row();

                        ui.label("Framing");
                        ui.add_enabled_ui(d.resolution != Resolution::Source, |ui| {
                            ui.horizontal(|ui| {
                                ui.radio_value(&mut d.framing, Framing::Fit, "Fit (bars if needed)");
                                ui.radio_value(&mut d.framing, Framing::Crop, "Crop to fill");
                            });
                        });
                        ui.end_row();

                        ui.label("Frame rate");
                        ui.horizontal(|ui| {
                            let default_fps = self
                                .project
                                .as_ref()
                                .map(|p| p.file.fps)
                                .unwrap_or(dragonslayer_core::project::DEFAULT_FPS);
                            let mut override_on = d.fps.is_some();
                            if ui
                                .checkbox(&mut override_on, "")
                                .on_hover_text("Override the project frame rate for this compile")
                                .changed()
                            {
                                d.fps = override_on.then_some(default_fps);
                            }
                            if let Some(fps) = &mut d.fps {
                                egui::ComboBox::from_id_salt("compile fps")
                                    .selected_text(format!("{fps} fps"))
                                    .show_ui(ui, |ui| {
                                        for choice in [6, 8, 10, 12, 15, 18, 24, 25, 30, 48, 60] {
                                            ui.selectable_value(fps, choice, format!("{choice} fps"));
                                        }
                                    });
                                ui.label(
                                    RichText::new(format!(
                                        "(project is {default_fps} fps; scene overrides ignored)"
                                    ))
                                    .small()
                                    .color(ui.visuals().weak_text_color()),
                                );
                            } else {
                                ui.label(
                                    RichText::new(format!("Use project ({default_fps} fps) and per-scene overrides"))
                                        .color(ui.visuals().weak_text_color()),
                                );
                            }
                        });
                        ui.end_row();
                    });
                });

                ui.add_space(8.0);
                if busy {
                    let p = f32::from_bits(d.progress.load(Ordering::Relaxed));
                    let elapsed = d.started.elapsed().as_secs_f32();
                    // ETA is noise for the first few percent while ffmpeg warms up.
                    let text = if p > 0.03 && p < 1.0 {
                        format!("{:.0}%  ·  about {} left", p * 100.0, human_secs(elapsed * (1.0 - p) / p))
                    } else {
                        format!("{:.0}%", p * 100.0)
                    };
                    ui.add(egui::ProgressBar::new(p).desired_width(ui.available_width()).animate(true).text(text));
                } else if ui
                    .add(
                        egui::Button::new(RichText::new("Compile").strong().size(15.0).color(crate::theme::palette().on_accent))
                            .fill(crate::theme::palette().accent)
                            .min_size(Vec2::new(140.0, 34.0)),
                    )
                    .clicked()
                    && let Some(p) = self.project.clone() {
                        let scene = (d.scope == Scope::Scene).then(|| p.file.active_scene.clone()).flatten();
                        let settings = Settings {
                            format: d.format,
                            resolution: d.resolution,
                            framing: d.framing,
                            fps_override: d.fps,
                            ffmpeg: None,
                        };
                        let (tx, rx) = mpsc::channel();
                        let ctx = ui.ctx().clone();
                        let progress = Arc::new(AtomicU32::new(0));
                        let shared = progress.clone();
                        thread::spawn(move || {
                            let r = compile::compile_with_progress(&p, scene.as_deref(), &settings, |f| {
                                shared.store(f.to_bits(), Ordering::Relaxed);
                            })
                            .map_err(|e| e.to_string());
                            let _ = tx.send(r);
                            ctx.request_repaint();
                        });
                        d.progress = progress;
                        d.started = Instant::now();
                        d.running = Some(rx);
                        d.result = None;
                    }

                match &d.result {
                    Some(Ok(out)) => {
                        ui.separator();
                        ui.label(format!("Saved {} frames to", out.frames));
                        ui.monospace(out.path.display().to_string());
                        for w in &out.warnings {
                            ui.label(RichText::new(format!("Warning: {w}")).small());
                        }
                        if ui.button("Show in folder").clicked() {
                            reveal(&out.path);
                        }
                    }
                    Some(Err(e)) => {
                        ui.separator();
                        ui.colored_label(Color32::from_rgb(220, 80, 70), e);
                    }
                    None => {}
                }
            });
        if !open && self.compile.running.is_none() {
            self.compile.open = false;
        }
    }

    fn help_window(&mut self, ctx: &egui::Context) {
        if !self.help_open {
            return;
        }
        // egui::Modal centres itself on the screen and doesn't persist window
        // position, so it can't drift off-screen the way our old Window did.
        let response = egui::Modal::new(egui::Id::new("help modal")).show(ctx, |ui| {
            ui.set_max_width(760.0);
            ui.set_max_height(680.0);
            ui.horizontal(|ui| {
                ui.heading("Help");
                ui.add_space(8.0);
                ui.selectable_value(&mut self.help_tab, HelpTab::Guide, "Guide");
                ui.selectable_value(&mut self.help_tab, HelpTab::Advanced, "Advanced camera troubleshooting");
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("Close").clicked() {
                        self.help_open = false;
                    }
                });
            });
            ui.horizontal(|ui| {
                ui.label(RichText::new("Instructions for:").small());
                ui.selectable_value(&mut self.help_os, HelpOs::Windows, "Windows");
                ui.selectable_value(&mut self.help_os, HelpOs::Mac, "macOS");
            });
            ui.separator();
            egui::ScrollArea::vertical().id_salt(self.help_tab).auto_shrink([false, false]).show(ui, |ui| {
                match self.help_tab {
                    HelpTab::Guide => help_content(ui, self.help_os),
                    HelpTab::Advanced => help_advanced(ui, self.help_os),
                }
            });
        });
        // Click outside the modal or press Escape → close.
        if response.should_close() {
            self.help_open = false;
        }
    }

    /// Camera diagnosis: each check with a verdict and, when it fails, the fix.
    fn diagnose_window(&mut self, ctx: &egui::Context) {
        if !self.diagnose_open {
            return;
        }
        use egui_phosphor::regular as ph;
        #[derive(Clone, Copy)]
        enum V {
            Ok,
            Warn,
            Fail,
            Info,
        }
        let pal = crate::theme::palette();
        let row = |ui: &mut egui::Ui, v: V, title: &str, fix: &str| {
            let (icon, color) = match v {
                V::Ok => (ph::CHECK_CIRCLE, pal.ok),
                V::Warn => (ph::WARNING, pal.warn),
                V::Fail => (ph::X_CIRCLE, pal.error),
                V::Info => (ph::INFO, pal.text_muted),
            };
            ui.horizontal_top(|ui| {
                ui.label(RichText::new(icon).size(16.0).color(color));
                ui.vertical(|ui| {
                    ui.label(RichText::new(title).strong());
                    if !fix.is_empty() {
                        ui.label(RichText::new(fix).color(pal.text_muted));
                    }
                });
            });
            ui.add_space(4.0);
        };

        let mut open = true;
        let mut rescan = false;
        let mut advanced = false;
        let mut any_wrong_driver = matches!(self.status, Status::WrongDriver { .. });
        egui::Window::new(format!("{}  Camera diagnosis", ph::STETHOSCOPE))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(520.0)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.set_max_width(520.0);
                ui.label(RichText::new("Camera library").small().color(pal.text_dim));
                let reset = "Do the full reset: quit DragonSlayer, turn the camera off (battery out if it \
                    won't respond), turn it on, then reopen DragonSlayer.";
                match &self.status {
                    Status::Connected { name, .. } => row(ui, V::Ok, &format!("DragonSlayer is talking to the {name}"), ""),
                    Status::Searching => row(ui, V::Fail, "No camera found", connect_hint()),
                    Status::WrongDriver { name } => row(
                        ui,
                        V::Fail,
                        &format!("{} is on Windows' own driver", name.as_deref().unwrap_or("The camera")),
                        "This USB port hasn't been set up for DragonSlayer. Click Set up USB driver… and \
                        replace the driver with WinUSB, or move the camera back to the port you set up before.",
                    ),
                    Status::Problem { name, message } => row(
                        ui,
                        V::Fail,
                        &format!("{}: {message}", name.as_deref().unwrap_or("Camera")),
                        reset,
                    ),
                    Status::NoBackend => row(
                        ui,
                        V::Fail,
                        "This build has no camera support",
                        "Download the release build from GitHub, or rebuild with --features gphoto2.",
                    ),
                }

                if let Status::Connected { caps, .. } = &self.status {
                    if self.camera_settings.is_empty() {
                        row(ui, V::Info, "The camera didn't report any settings", "Some models don't; capture still works.");
                    } else {
                        row(
                            ui,
                            V::Ok,
                            &format!("The camera answers commands ({} settings read)", self.camera_settings.len()),
                            "",
                        );
                    }
                    let live_recent = self.last_live_at.is_some_and(|t| t.elapsed() < Duration::from_secs(3));
                    if !caps.live_view {
                        row(ui, V::Info, "This model has no live view over USB", "Onion skin shows over the last frame instead.");
                    } else if !self.mode.is_capture() {
                        row(ui, V::Info, "Live view is paused while you're in Preview", "Press Tab to go back to Capture.");
                    } else if live_recent {
                        row(ui, V::Ok, "Live view frames are arriving", "");
                    } else {
                        row(
                            ui,
                            V::Fail,
                            "No live view frames",
                            "Canon: mode dial on M and Live View shooting enabled in the menu. Check the battery. \
                            If it still fails, do the full reset (quit DragonSlayer first).",
                        );
                    }
                }

                ui.add_space(6.0);
                ui.label(RichText::new("USB").small().color(pal.text_dim));
                if !cfg!(windows) {
                    row(
                        ui,
                        V::Info,
                        "USB driver checks are for Windows",
                        "On macOS: quit Photos and Image Capture. If the camera was plugged in after \
                        DragonSlayer opened, quit and reopen DragonSlayer.",
                    );
                } else if self.usb_scan_rx.is_some() {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Checking USB devices…");
                    });
                } else {
                    match &self.usb_scan {
                        None => {}
                        Some(Err(e)) => row(ui, V::Info, "Couldn't check the USB devices", e),
                        Some(Ok(cams)) if cams.is_empty() => row(
                            ui,
                            V::Fail,
                            "No camera seen on USB at all",
                            "Is the camera on? Some USB cables only charge: try another cable. Check the \
                            camera's USB mode is PC / PTP, not mass storage.",
                        ),
                        Some(Ok(cams)) => {
                            for c in cams {
                                match &c.driver {
                                    dragonslayer_camera::diag::Driver::Usable(d) => {
                                        row(ui, V::Ok, &format!("{}: driver {d}", c.name), "")
                                    }
                                    dragonslayer_camera::diag::Driver::WindowsOwn(d) => {
                                        any_wrong_driver = true;
                                        row(
                                            ui,
                                            V::Fail,
                                            &format!("{} is on Windows' own driver ({d})", c.name),
                                            "Zadig's setup belongs to one USB port, and this one hasn't been set \
                                            up. Click Set up USB driver… (Options → List All Devices, pick the \
                                            camera, WinUSB, Replace Driver), or move the camera back to the port \
                                            you set up before.",
                                        );
                                    }
                                }
                                match &c.hub {
                                    Some(hub) => row(
                                        ui,
                                        V::Warn,
                                        &format!("{} is plugged into a hub ({hub})", c.name),
                                        "Hubs are the most common cause of timeouts. If the camera drops or \
                                        times out, plug it straight into the computer (and run Zadig once for \
                                        that port).",
                                    ),
                                    None => row(ui, V::Ok, &format!("{} is plugged straight into the computer", c.name), ""),
                                }
                            }
                        }
                    }
                }

                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button(format!("{}  Check again", ph::ARROW_CLOCKWISE)).clicked() {
                        rescan = true;
                    }
                    if cfg!(windows) && any_wrong_driver && ui.button("Set up USB driver…").clicked() {
                        launch_driver_setup();
                    }
                    if ui.button("Advanced troubleshooting").clicked() {
                        advanced = true;
                    }
                });
            });
        if rescan {
            self.start_usb_scan(ctx);
        }
        if advanced {
            self.help_open = true;
            self.help_tab = HelpTab::Advanced;
            self.diagnose_open = false;
        }
        if !open {
            self.diagnose_open = false;
        }
    }
}

/// Deeper diagnosis for when the Guide's troubleshooting hasn't fixed the camera.
fn help_advanced(ui: &mut egui::Ui, os: HelpOs) {
    let h = |ui: &mut egui::Ui, s: &str| {
        ui.add_space(8.0);
        ui.heading(s);
    };
    let sub = |ui: &mut egui::Ui, s: &str| {
        ui.add_space(6.0);
        ui.label(RichText::new(s).strong());
    };
    let p = |ui: &mut egui::Ui, s: &str| {
        ui.label(s);
        ui.add_space(2.0);
    };
    let code = |ui: &mut egui::Ui, s: &str| {
        ui.add(egui::Label::new(RichText::new(s).monospace().background_color(crate::theme::palette().bg_elevated)).wrap());
        ui.add_space(2.0);
    };
    let windows = os == HelpOs::Windows;

    p(ui, "Start with Help → Diagnose camera… (or the stethoscope button next to the camera \
        status). It runs these checks for you and says which one fails. The rest of this page \
        is for when that isn't enough; most camera problems are one of the first three sections.");

    h(ui, "1. Read the message");
    p(ui, "The status bar (top right) says what the camera last reported. What it usually means:");
    sub(ui, "\"Windows is still using its own driver\" / \"Could not claim the USB device\"");
    p(ui, if windows {
        "Windows has the camera on its own photo driver, not WinUSB. Almost always because \
        the camera is on a different USB port than the one Zadig was run on, or a Windows \
        update reset it. Run Zadig again for this port (section 2 shows how to check)."
    } else {
        "Another program has the camera: macOS's ptpcamerad, Image Capture, Photos, or a \
        vendor app. See section 6."
    });
    sub(ui, "\"I/O error\", \"PTP Timeout\", or live view dies straight away");
    p(ui, "The camera stopped answering. Usually a stuck USB session left over from a crash \
        or a disconnect mid-transfer: do the full reset in section 3. If it comes straight \
        back after a reset, it's the USB connection itself: see section 4.");
    sub(ui, "\"Camera disconnected\"");
    p(ui, "The USB link dropped: cable knocked, camera slept or battery died. Check section 5, \
        then reconnect. Frames already captured are safe; one caught mid-download is \
        recovered on the next launch or is still on the camera card.");
    sub(ui, "Connected, but some settings are missing from the Exposure panel");
    p(ui, "Camera brands name settings differently and DragonSlayer may not know your \
        camera's name for one yet. Please report it (section 8).");

    if windows {
        h(ui, "2. Check which driver Windows is using");
        p(ui, "Open Device Manager (right-click Start → Device Manager) with the camera on and \
            plugged in:");
        p(ui, "· Under \"Universal Serial Bus devices\" → WinUSB. This is what DragonSlayer needs.");
        p(ui, "· Under \"Portable Devices\" (or \"Cameras\") → Windows' own driver. Run Zadig: \
            Options → List All Devices, pick the camera, WinUSB, Replace Driver.");
        p(ui, "Zadig's change applies to one USB port. Pick one port for the camera, run \
            Zadig with it there, and always use that port.");
    } else {
        h(ui, "2. Check macOS isn't holding the camera");
        p(ui, "DragonSlayer quits macOS's camera service (ptpcamerad) when it starts, but it \
            restarts itself and grabs cameras plugged in later. If the camera was plugged in \
            after DragonSlayer opened, quit DragonSlayer and open it again. From Terminal:");
        code(ui, "killall ptpcamerad");
        p(ui, "In Image Capture, select the camera and set \"Connecting this camera opens\" to \
            \"No application\" so Photos stops launching.");
    }

    h(ui, "3. The full reset, in this order");
    p(ui, "Order matters. Resetting the camera while DragonSlayer still has it open just \
        wedges it again.");
    p(ui, "1. Quit DragonSlayer (and any other camera software).");
    p(ui, "2. Turn the camera off. If it doesn't respond, take the battery out for 10 seconds.");
    p(ui, "3. Turn it back on and wait until its screen is up.");
    p(ui, "4. Open DragonSlayer.");
    p(ui, "A stuck session never damages the camera. It's software state, and the reset \
        clears it.");

    h(ui, "4. USB: hubs, cables and ports");
    p(ui, "If the camera times out again right after a clean reset, the connection is the \
        likely culprit:");
    p(ui, "· Hubs: plug straight into the computer. Unpowered hubs, monitor USB ports and \
        dongles with many ports are the most common cause of timeouts.");
    p(ui, "· Cables: use a short data cable. Some cables only charge; long ones drop out.");
    p(ui, "· Ports: prefer a port on the back of a desktop (on the motherboard).");
    if windows {
        p(ui, "Changing port on Windows means running Zadig again for the new port.");
    }

    h(ui, "5. Power");
    p(ui, "· Turn auto power off / sleep off in the camera's menu while shooting.");
    p(ui, "· A low battery kills live view before it stops the camera taking photos. For \
        long shoots, a mains adapter (dummy battery) for your camera is worth having.");
    p(ui, "· Live view keeps the sensor on and warms the camera. If it overheats it shuts \
        down to protect itself: let it cool, then carry on.");

    h(ui, "6. Camera-specific");
    sub(ui, "Canon EOS (e.g. 100D)");
    p(ui, "· Mode dial on M. Scene and green auto modes can refuse remote live view.");
    p(ui, "· Red camera menu → Live View shooting: Enable. Without it, live view never starts.");
    p(ui, "· Movie mode blocks stills capture. Use the stills position.");
    p(ui, "· The camera shows a computer icon while connected. That's normal: the picture \
        appears in DragonSlayer, not on the camera's screen.");
    sub(ui, "Panasonic Lumix (e.g. GH5)");
    p(ui, "· Menu → Setup → USB Mode → PC(Tether).");
    p(ui, "· Panasonic bodies can lock up under long live-view sessions. DragonSlayer pauses \
        live view in Preview mode to give the camera a rest. If it keeps happening, switch \
        to Preview between shots.");
    if !windows {
        sub(ui, "macOS");
        p(ui, "· Quit Photos, Image Capture and vendor apps before opening DragonSlayer.");
    }

    h(ui, "7. Test the camera without the app");
    p(ui, "The command-line tool talks to the camera directly, which tells you whether the \
        problem is the camera or the app. Quit DragonSlayer first.");
    if windows {
        p(ui, "In a Command Prompt in the DragonSlayer folder:");
        code(ui, "DragonSlayer-CLI.cmd diagnose");
    } else {
        p(ui, "In Terminal:");
        code(ui, "R=/Applications/DragonSlayer.app/Contents; CAMLIBS=$R/Resources/libgphoto2/camlibs IOLIBS=$R/Resources/libgphoto2/iolibs $R/MacOS/dragonslayer-cli diagnose");
    }
    p(ui, "\"diagnose\" checks the USB driver and hub, opens the camera, reads its settings and \
        grabs a live view frame, printing ok or x for each step. Swap it for \"cameras\" or \
        \"settings\" to see just those.");

    h(ui, "8. Reporting a problem");
    p(ui, "Open an issue on GitHub with your camera model, operating system, what you did and \
        what happened, and the last lines of the log:");
    code(ui, if windows { "%TEMP%\\dragonslayer.log" } else { "~/Library/Logs/dragonslayer.log" });
    p(ui, "Include the output of \"diagnose\" from section 7, and of \"settings\" for missing \
        Exposure settings.");
}

fn help_content(ui: &mut egui::Ui, os: HelpOs) {
    let h = |ui: &mut egui::Ui, s: &str| {
        ui.add_space(6.0);
        ui.heading(s);
    };
    let p = |ui: &mut egui::Ui, s: &str| {
        ui.label(s);
        ui.add_space(2.0);
    };
    let k = |ui: &mut egui::Ui, key: &str, what: &str| {
        ui.horizontal(|ui| {
            // Explicit light text on dark chip: the previous version had no colour set
            // and inherited whatever the ambient style decided, which came out black on
            // pale grey inside the modal frame.
            ui.label(
                RichText::new(format!(" {key} "))
                    .monospace()
                    .color(crate::theme::palette().text_primary)
                    .background_color(crate::theme::palette().bg_elevated),
            );
            ui.label(RichText::new(what).color(crate::theme::palette().text_primary));
        });
    };

    h(ui, "What DragonSlayer does");
    p(ui, "DragonSlayer tethers your DSLR or mirrorless camera over USB, shoots your film scene \
        by scene with onion skinning, and compiles it into a video. Nothing else. It's meant to \
        be simple enough that a first-time student can pick it up without a manual — this Help \
        panel exists for the setup bits that are one-time.");

    h(ui, "Get started");
    p(ui, "1. Click New project… and pick a folder. DragonSlayer creates it with one empty scene.");
    p(ui, "2. Plug in your camera on USB, in PC / PC(Tether) / PTP mode. The dot top-right \
        should go green with the camera's name.");
    p(ui, "3. Press Space to capture. The frame lands in the active scene, the counter goes up, \
        and the thumbnail on the left updates.");
    p(ui, "4. Move your subject a tiny bit, press Space again. Onion skin shows where the \
        previous frame was, so you know how far to move.");
    p(ui, "5. When the scene is done, click + Add scene and keep going.");
    p(ui, "6. Click Compile… to render an MP4 or ProRes MOV.");

    h(ui, "Keyboard shortcuts");
    k(ui, "Space", "Capture the next frame into the active scene (Capture mode only)");
    k(ui, "Backspace", "Move the last frame to that scene's trash folder (undo delete by \
        moving it back from disk — nothing is destroyed)");
    k(ui, "O", "Toggle onion skin on/off");
    k(ui, "Tab", "Switch between Capture and Preview");
    k(ui, "← / →", "Previous / next frame (Shift jumps 10); Home / End for first / last");
    k(ui, "P", "Play / pause at the scene's frame rate");
    k(ui, "F", "Minimal view: full-screen capture with a small floating control bar");
    k(ui, "Esc", "Leave minimal view, or go back to Capture");

    h(ui, "Scene list");
    p(ui, "· Click a scene to make it active. Captures go into the active scene.");
    p(ui, "· Double-click to rename. Enter saves, Esc cancels.");
    p(ui, "· Drag a scene up or down to reorder it in the film.");
    p(ui, "· Right-click for frame rate override and delete. Deleted scenes go to the project \
        trash folder — recover them by moving them back.");
    p(ui, "· + Add scene inserts a new scene after the active one.");

    h(ui, "Onion skin");
    p(ui, "Ghosts of previous frames overlaid on the live view, so you know where your subject \
        was on the last shot. Frames slider: 1 to 5 previous frames. Opacity: how prominent the \
        ghosts are. Outlines: show only edges of the previous frame instead of the whole ghost, \
        which keeps the live view clear.");

    h(ui, "Exposure (camera settings)");
    p(ui, "The Exposure panel changes aperture, shutter, ISO, white balance and image format \
        on the camera, so you don't have to touch it (and knock the shot) between frames. \
        Put the mode dial on M: in auto modes the camera locks some settings (they show \
        greyed out) and changes exposure between frames, which makes the film flicker. \
        Set Image format to RAW + JPEG there to keep both files.");

    h(ui, "Interval capture");
    p(ui, "In the Camera panel: capture N frames automatically, a set number of seconds \
        apart, for time-lapses. Each shot waits for the previous one to finish, so a slow \
        camera just stretches the schedule. Stop ends it at any time; it also stops by \
        itself if the camera disconnects or you switch to Preview.");

    h(ui, "Compile");
    p(ui, "Compile… asks whether to render this scene or the whole project, what format \
        (H.264 MP4 or ProRes 422 MOV), what resolution (source, 4K, 1080p) and how to frame \
        (fit with black bars, or crop to fill). A progress bar shows how far along it is and \
        roughly how long is left. Videos land in your project's exports/ folder and are never \
        overwritten.");

    match os {
        HelpOs::Windows => {
            h(ui, "Windows: one-time setup");
            p(ui, "Windows binds cameras to its own driver by default. DragonSlayer needs libusb, \
                which needs the WinUSB driver, which needs a one-time swap with Zadig.");
            p(ui, "1. Download Zadig from https://zadig.akeo.ie (single .exe, no install).");
            p(ui, "2. Plug in the camera, turn it on, set it to PC / PC(Tether) / PTP mode.");
            p(ui, "3. In Zadig: Options → List All Devices.");
            p(ui, "4. Pick your camera from the dropdown (for example DC-GH5 or EOS 100D).");
            p(ui, "5. Target driver: WinUSB. Click Replace Driver. Wait ~30 s.");
            p(ui, "Nothing on the camera changes — only the Windows setting for which driver \
                claims that USB port. To undo: Device Manager → the camera under Universal \
                Serial Bus devices → Uninstall device → tick \"delete the driver software\" → \
                unplug/replug.");
            p(ui, "While swapped, the Windows Photos app and vendor tools won't see the \
                camera. That's expected.");
            p(ui, "Zadig's change belongs to one USB port. Plug the camera into the same port \
                every time: on a different port Windows sees a new device, puts its own driver \
                back, and you'd need to run Zadig again. Use a port on the computer itself \
                rather than a hub, which can make the camera time out.");
        }
        HelpOs::Mac => {
            h(ui, "macOS: releasing the camera");
            p(ui, "macOS grabs cameras for Image Capture and Photos. Before DragonSlayer can \
                talk to yours:");
            p(ui, "1. Quit Image Capture and Photos.");
            p(ui, "2. In Terminal, run:  killall ptpcamerad");
            p(ui, "3. Reconnect the camera and open DragonSlayer.");
            p(ui, "If DragonSlayer says the camera is busy, run killall ptpcamerad again — the \
                process restarts on its own.");
        }
    }

    h(ui, "Camera USB mode");
    p(ui, "Cameras have several USB modes. DragonSlayer needs PC control, not mass-storage.");
    p(ui, "· Panasonic (e.g. GH5): Menu → SETUP → USB Mode → PC(Tether).");
    p(ui, "· Canon EOS (e.g. 100D): works as soon as it's plugged in. For live view, set the \
        mode dial to M and check Live View shooting is enabled in the red camera menu. \
        The camera shows a small computer icon while connected; that's normal, the \
        picture appears in DragonSlayer.");
    p(ui, "· Set RAW+JPEG on the camera if you want both files kept. If off, DragonSlayer \
        keeps the JPEG and prints a one-off warning.");

    h(ui, "Troubleshooting");

    let sub = |ui: &mut egui::Ui, s: &str| {
        ui.add_space(6.0);
        ui.label(RichText::new(s).strong());
    };

    sub(ui, "\"No camera found\" but the camera is plugged in");
    p(ui, "The most common cause is a wedged PTP session — a previous session (DragonSlayer or \
        anything else) crashed while the camera was open, and the camera thinks it's still \
        talking to a host that isn't there. Nothing on the camera is broken; it just needs a \
        reset.");
    p(ui, "Fix in order:");
    p(ui, "1. Turn the camera off, wait 3 seconds, turn it back on. This is the fix for \
        a wedged camera 90% of the time.");
    p(ui, "2. If still nothing: unplug the USB, wait 3 seconds, plug it back into the same \
        port. (On Windows, a different port needs Zadig again; see the Windows setup section.)");
    p(ui, "3. Check the camera's USB mode (see the \"Camera USB mode\" section above).");
    p(ui, "4. Close any other program that might have grabbed the camera: Photos, Image \
        Capture, Canon EOS Utility, Lumix Tether, etc.");
    match os {
        HelpOs::Windows => {
            p(ui, "5. On Windows: check that Zadig was applied to THIS USB port (Device \
                Manager should show the camera under \"Universal Serial Bus devices\", not \
                under \"Portable Devices\" or \"Imaging devices\").");
        }
        HelpOs::Mac => {
            p(ui, "5. On macOS: open Terminal and run:  killall ptpcamerad  — this stops \
                the macOS camera daemon from grabbing the connection. The daemon restarts on \
                its own if needed.");
        }
    }

    sub(ui, "\"Camera in use by another program\"");
    p(ui, "Something else already has the USB interface open. Close: the vendor's own \
        tether app (Canon EOS Utility, Lumix Tether, Nikon Camera Control), Windows Photos, \
        macOS Photos and Image Capture, any Adobe Lightroom import wizard, browser tabs \
        showing camera previews. Unplug and replug after closing.");
    if matches!(os, HelpOs::Mac) {
        p(ui, "On macOS the most common culprit is ptpcamerad, a background process. Run:  \
            killall ptpcamerad  — it restarts itself but releases the camera in between.");
    }

    sub(ui, "Capture fails with a timeout, works after camera reboot");
    p(ui, "Same as \"wedged camera\" above: the PTP session got out of sync. Camera off, \
        3 s, camera on. If it keeps happening on one specific model, please report the \
        camera and the exact error text.");

    sub(ui, "Capture takes 3–20 seconds per frame");
    p(ui, "Not a bug for some bodies. Panasonic in particular is slow through libgphoto2's \
        PTP path — the camera itself takes several seconds per shot. Canon is usually 1–3 s, \
        Nikon 2–5 s. What you can improve:");
    p(ui, "· Turn off RAW+JPEG in the camera if you don't need RAW. RAW downloads take \
        significantly longer.");
    p(ui, "· Reduce the JPEG size on the camera (Fine → Normal, or Large → Medium).");
    p(ui, "· Use a fast USB-A port directly on the computer, not through a hub.");

    sub(ui, "Live view stops but capture still works");
    p(ui, "Panasonic bodies sometimes exit live view after a capture and don't recover on \
        their own. Toggle Live/Last frame (Tab) once — this restarts the request. If it \
        happens every capture, check the camera doesn't have an auto power-off setting \
        that's cutting live view.");

    sub(ui, "Camera keeps going to sleep");
    p(ui, "Every DSLR has an auto power-off. During long stop-motion sessions this will \
        interrupt tethering. In the camera menu, set auto power-off to \"Off\" or the \
        longest available setting. DragonSlayer already prevents the computer from sleeping \
        while the app is open with a project.");

    sub(ui, "Live view runs but no live view is shown");
    p(ui, "Not every camera supports USB live view (older DSLRs often don't). If the \
        model reports no live view at connect, DragonSlayer shows the last captured frame \
        instead — press Space to capture, and the viewer will follow along. Onion skin \
        still works, layered on the last frame.");

    sub(ui, "Onion skin looks muddy or doesn't help");
    p(ui, "Turn Outlines on — that shows only the edges of the previous frame, which \
        makes movement much clearer than a full ghost. If you shoot on a bright \
        background and outlines are noisy, drop the opacity slider a notch.");

    sub(ui, "Frames aren't appearing in scenes/");
    p(ui, "Check the message bar at the bottom of the app for an error. If capture is \
        succeeding (the counter goes up, thumbnails update) but you can't find the files, \
        make sure you're looking at the correct project — the top bar shows its name — \
        and the correct scene folder inside scenes/. Deleted frames go to \
        scenes/scXXX/trash/, not out of existence.");

    sub(ui, "The app won't start");
    match os {
        HelpOs::Windows => {
            p(ui, "If Windows shows \"the program can't start because <name>.dll is missing\", \
                the DLLs weren't next to the .exe. Reinstall by copying the whole target/ \
                folder or use the .cmd wrapper in bin/.");
            p(ui, "If clicking the .cmd flashes a window then closes, an old dragonslayer-app.exe \
                is still running. Open Task Manager, kill any \"dragonslayer-app.exe\" entries, \
                and try again. The wrapper now kills any stale one automatically.");
        }
        HelpOs::Mac => {
            p(ui, "If macOS says the app is damaged or from an unidentified developer, run:  \
                xattr -dr com.apple.quarantine /path/to/dragonslayer-app  — this removes the \
                quarantine attribute Safari adds to downloaded binaries.");
        }
    }

    sub(ui, "Recovering from a crash");
    p(ui, "Nothing on disk is destroyed. Every capture is a transaction — the file is \
        flushed to disk before it's recorded in the journal. On the next launch DragonSlayer \
        replays the journal, moves any file left in the incoming/ folder into frames/, \
        and marks anything with no file as abandoned (usually still on the camera card). \
        You should see a brief \"Recovered N captures\" message at the bottom.");

    sub(ui, "\"No camera support in this build\"");
    p(ui, "The app was compiled without the gphoto2 feature. Rebuild with:  \
        cargo build --release --features gphoto2 -p dragonslayer-app  — or launch with \
        --mock to try the built-in fake camera.");

    h(ui, "The project folder");
    p(ui, "A project is a plain folder — you can copy it to a USB stick or hand it in. \
        Frames live under scenes/scXXX/frames/. Deleted frames and scenes go to trash \
        folders next to them, never destroyed. journal.ndjson records every capture and \
        delete so the frame order can be rebuilt from disk.");

    h(ui, "About this build");
    p(ui, &format!(
        "DragonSlayer v{}. Free and open source. Report bugs, ask for cameras to be added, \
        or contribute at https://github.com/TheMagnificentRonnie/DragonSlayer",
        env!("CARGO_PKG_VERSION")
    ));

    h(ui, "Disclaimer — please read");
    ui.label(
        RichText::new(
            "DragonSlayer is free, open-source hobby software. It is provided \"as is\", \
             with no warranty of any kind — express or implied — including but not limited \
             to fitness for purpose, merchantability, or non-infringement. Use it entirely \
             at your own risk.",
        )
        .strong(),
    );
    ui.add_space(4.0);
    p(ui, "By using this software you accept that:");
    p(ui, "· The authors and contributors are not liable for any lost footage, damaged \
        files, missed shots, corrupt SD cards, wedged cameras, missed deadlines, delayed \
        productions, or any other direct, indirect, incidental, special, exemplary or \
        consequential damages arising from use or misuse of this software.");
    p(ui, "· Nothing about DragonSlayer is professionally supported. There is no help \
        desk. There is no SLA. Bug fixes happen when someone in the community writes them.");
    p(ui, "· This is a BETA. Features may change, break, or disappear. The project format \
        should stay compatible, but that is not a guarantee.");
    p(ui, "· For anything mission-critical — a paid gig, an assessed project, an \
        irreplaceable shot — back up frequently, keep the camera card, and consider \
        using established commercial software as well or instead.");
    p(ui, "· Camera firmware, USB drivers and OS updates can break tethering in ways \
        outside this project's control. If your camera stops responding, the fix is \
        usually to power-cycle it (see Troubleshooting → wedged camera above).");
    p(ui, "· The bundled Zadig helper installs a WinUSB driver on Windows. This is a \
        Windows setting change, not a camera modification, and can be undone in Device \
        Manager. While active, the Windows Photos app and vendor tools will not see the \
        camera.");
    p(ui, "· If you don't agree to any of this, don't use the software. Delete it, \
        keep your camera card, and have a nice day.");
    ui.add_space(12.0);
}

enum SceneAction {
    Activate(String),
    Add,
    Rename(String, String),
    Fps(String, Option<u32>),
    Move(String, usize),
    Delete(String),
}

fn scene_row(
    ui: &mut egui::Ui,
    row: &SceneRow,
    active: bool,
    images: &mut Images,
    renaming: &mut Option<(String, String)>,
) -> egui::Response {
    let height = 58.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::click_and_drag());
    let visuals = ui.visuals().clone();
    let bg = if active {
        visuals.selection.bg_fill
    } else if resp.hovered() {
        visuals.widgets.hovered.weak_bg_fill
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, 4.0, bg);

    let thumb = Rect::from_min_size(rect.min + Vec2::new(4.0, 4.0), Vec2::new(80.0, height - 8.0));
    ui.painter().rect_filled(thumb, 2.0, Color32::from_gray(30));
    if let Some(tex) = row.last_jpeg.as_ref().and_then(|p| images.get(p, THUMB_WIDTH)) {
        let r = fit(thumb, tex.size_vec2());
        ui.painter().image(tex.id(), r, Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
    }

    let text_x = thumb.right() + 8.0;
    let text_color = if active { visuals.selection.stroke.color } else { visuals.text_color() };
    match renaming {
        Some((id, buf)) if *id == row.id => {
            let edit_rect = Rect::from_min_max(egui::pos2(text_x, rect.top() + 6.0), egui::pos2(rect.right() - 4.0, rect.top() + 28.0));
            let r = ui.put(edit_rect, egui::TextEdit::singleline(buf));
            r.request_focus();
        }
        _ => {
            ui.painter().text(
                egui::pos2(text_x, rect.top() + 10.0),
                Align2::LEFT_TOP,
                &row.name,
                FontId::proportional(15.0),
                text_color,
            );
        }
    }
    let fps = row.fps.map(|f| format!(" · {f} fps")).unwrap_or_default();
    ui.painter().text(
        egui::pos2(text_x, rect.top() + 32.0),
        Align2::LEFT_TOP,
        format!("{} frames{fps}", row.count),
        FontId::proportional(12.0),
        visuals.weak_text_color(),
    );
    resp
}

fn overlay(ui: &egui::Ui, rect: Rect, scene: &str, mode: &str) {
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

/// Largest rect with `size`'s aspect ratio centred in `area`.
/// UV sub-rectangle that makes an image of `size` cover `area` completely, cropping
/// whichever edges overhang (centred). The counterpart of `fit`.
fn cover_uv(area: Rect, size: Vec2) -> Rect {
    let full = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    if size.x <= 0.0 || size.y <= 0.0 || area.width() <= 0.0 || area.height() <= 0.0 {
        return full;
    }
    let scale = (area.width() / size.x).max(area.height() / size.y);
    let visible = Vec2::new(area.width() / scale / size.x, area.height() / scale / size.y);
    Rect::from_center_size(egui::pos2(0.5, 0.5), visible)
}

fn fit(area: Rect, size: Vec2) -> Rect {
    if size.x <= 0.0 || size.y <= 0.0 {
        return area;
    }
    let scale = (area.width() / size.x).min(area.height() / size.y);
    Rect::from_center_size(area.center(), size * scale)
}

fn connect_hint() -> &'static str {
    if cfg!(windows) {
        "Turn the camera on, set it to PC/PTP mode and plug it in. First time on Windows? Run Zadig (see README)."
    } else if cfg!(target_os = "macos") {
        "Turn the camera on and plug it in. Quit Photos and Image Capture; if it still isn't found, run `killall ptpcamerad`."
    } else {
        "Turn the camera on, set it to PC/PTP mode and plug it in."
    }
}

fn human_secs(secs: f32) -> String {
    let s = secs.round() as u64;
    if s < 60 { format!("{s}s") } else { format!("{}m {:02}s", s / 60, s % 60) }
}

/// Windows-only: launch bundled Zadig if it's next to our exe, else open the download page.
/// Zadig itself asks for admin (UAC) and does the WinUSB install. This is a stepping stone
/// to a fully in-app installer using libwdi.
#[cfg(windows)]
fn launch_driver_setup() {
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
fn launch_driver_setup() {}

/// Simple debug log to %TEMP%\dragonslayer.log (Windows) or ~/Library/Logs/dragonslayer.log (macOS)
/// so we can see what an off-machine build actually does.
fn log_line(msg: &str) {
    use std::io::Write;
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

fn chrono_stamp() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    let s = now % 60;
    let m = (now / 60) % 60;
    let h = (now / 3600) % 24;
    format!("[{h:02}:{m:02}:{s:02}]")
}

fn reveal(path: &Path) {
    let _ = if cfg!(windows) {
        std::process::Command::new("explorer").arg(format!("/select,{}", path.display())).spawn()
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg("-R").arg(path).spawn()
    } else {
        std::process::Command::new("xdg-open").arg(path.parent().unwrap_or(path)).spawn()
    };
}

impl egui_dock::TabViewer for DragonSlayerApp {
    type Tab = Tab;

    fn id(&mut self, tab: &mut Self::Tab) -> egui::Id {
        egui::Id::new(("dragonslayer-tab", *tab))
    }

    fn title(&mut self, tab: &mut Self::Tab) -> egui::WidgetText {
        use egui_phosphor::regular as ph;
        let s = match tab {
            Tab::Scenes => format!("{}  Scenes", ph::FILM_STRIP),
            Tab::Viewer => format!("{}  Viewer", ph::IMAGE_SQUARE),
            Tab::Timeline => format!("{}  Timeline", ph::LIST_BULLETS),
            Tab::Camera => format!("{}  Camera", ph::CAMERA),
            Tab::Exposure => format!("{}  Exposure", ph::APERTURE),
            Tab::Onion => format!("{}  Onion Skin", ph::STACK),
            Tab::Export => format!("{}  Export", ph::EXPORT),
        };
        s.into()
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Self::Tab) {
        match tab {
            Tab::Scenes => self.tab_scenes(ui),
            Tab::Viewer => self.viewer(ui),
            Tab::Timeline => self.tab_timeline(ui),
            Tab::Camera => self.tab_camera(ui),
            Tab::Exposure => self.tab_exposure(ui),
            Tab::Onion => self.tab_onion(ui),
            Tab::Export => self.tab_export(ui),
        }
    }

    fn is_closeable(&self, _tab: &Self::Tab) -> bool {
        // Users can drag/rearrange but not close (no reopen UI yet).
        false
    }

    fn clear_background(&self, tab: &Self::Tab) -> bool {
        // We paint our own background inside the Viewer tab.
        *tab != Tab::Viewer
    }
}

impl eframe::App for DragonSlayerApp {
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        self.take_shortcuts(raw, ctx.text_edit_focused());
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.images.begin_frame(&ctx);
        if self.mode.is_capture() != self.logged_mode_is_capture {
            self.logged_mode_is_capture = self.mode.is_capture();
            log_line(&format!("mode -> {}", if self.logged_mode_is_capture { "Capture" } else { "Preview" }));
        }
        self.handle_events(&ctx);
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
