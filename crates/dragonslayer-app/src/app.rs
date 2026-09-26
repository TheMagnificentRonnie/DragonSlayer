use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align, Align2, Color32, FontId, Key, Layout, Rect, RichText, Sense, Stroke, TextureHandle,
    TextureOptions, Vec2,
};
use dragonslayer_core::compile::{self, Format, Framing, Resolution, Settings};
use dragonslayer_core::{Frame, Project};

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
    viewer: ViewerState,
    /// Wall-clock time of the last playback frame advance; also used as a repaint anchor.
    playback_tick: Instant,
    capturing: bool,

    onion_on: bool,
    onion_count: usize,
    onion_opacity: f32,
    onion_edges: bool,

    images: Images,
    renaming: Option<(String, String)>,
    drag_from: Option<usize>,
    compile: CompileDialog,
    help_open: bool,
    help_os: HelpOs,
    message: Option<(String, bool, Instant)>,
    keys: Vec<Key>,
    _awake: Option<keepawake::KeepAwake>,
}

#[derive(Clone, Copy, PartialEq)]
enum HelpOs {
    Windows,
    Mac,
}

/// What the viewer is showing.
#[derive(Clone, Copy, PartialEq, Debug)]
enum ViewerState {
    /// Live view from the camera (or "no live view" placeholder if not available).
    Live,
    /// A specific captured frame from the active scene, by index.
    Frame(usize),
    /// Playing frames back at the scene's fps. `index` is the current frame,
    /// `last_advance` is when we last moved forward.
    Playing { index: usize, last_advance: Instant },
}

impl ViewerState {
    fn is_live(self) -> bool {
        matches!(self, ViewerState::Live)
    }
    fn is_playing(self) -> bool {
        matches!(self, ViewerState::Playing { .. })
    }
    /// The frame index the viewer is anchored to, if any.
    fn frame_index(self) -> Option<usize> {
        match self {
            ViewerState::Live => None,
            ViewerState::Frame(i) | ViewerState::Playing { index: i, .. } => Some(i),
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
            viewer: ViewerState::Live,
            playback_tick: Instant::now(),
            capturing: false,
            onion_on: true,
            onion_count: 1,
            onion_opacity: 0.55,
            onion_edges: true,
            images: Images::new(&ctx),
            help_open: false,
            help_os: if cfg!(target_os = "macos") { HelpOs::Mac } else { HelpOs::Windows },
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
        if self.capturing || !self.camera_ready() {
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
                    }
                    self.status = s;
                }
                Event::Live(img) => match &mut self.live {
                    Some(t) if t.size() == img.size => t.set(img, TextureOptions::LINEAR),
                    _ => self.live = Some(ctx.load_texture("live view", img, TextureOptions::LINEAR)),
                },
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
                        Err(e) => self.error(format!("Capture failed: {e}")),
                    }
                }
            }
        }

        if let Some(rx) = &self.compile.running
            && let Ok(result) = rx.try_recv() {
                self.compile.running = None;
                self.compile.result = Some(result);
            }
    }

    fn shortcuts_active(&self) -> bool {
        self.renaming.is_none() && !self.compile.open && !self.help_open
    }

    /// Takes the shortcut keys out of the input before egui sees them, so a
    /// focused button can't swallow Space and Tab doesn't move keyboard focus.
    fn take_shortcuts(&mut self, raw: &mut egui::RawInput) {
        if !self.shortcuts_active() {
            return;
        }
        // Keys we handle ourselves. Arrow keys and Home/End are navigation.
        const KEYS: [Key; 11] = [
            Key::Space, Key::Backspace, Key::O, Key::Tab, Key::H, Key::P,
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
            egui::Event::Text(t) => !matches!(t.as_str(), " " | "o" | "O" | "h" | "H" | "p" | "P"),
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
                Key::Escape => self.viewer = ViewerState::Live,
                _ => {}
            }
        }
    }

    // ---- navigator ------------------------------------------------------

    /// Ctrl-Tab-style toggle between live view and the last captured frame.
    fn toggle_live_last(&mut self) {
        self.viewer = match self.viewer {
            ViewerState::Live => match self.frames.len().checked_sub(1) {
                Some(i) => ViewerState::Frame(i),
                None => ViewerState::Live,
            },
            _ => ViewerState::Live,
        };
    }

    fn step(&mut self, delta: i32) {
        if self.frames.is_empty() {
            return;
        }
        let last = self.frames.len() - 1;
        let current = self.viewer.frame_index().unwrap_or(last);
        let next = (current as i64 + delta as i64).clamp(0, last as i64) as usize;
        self.viewer = ViewerState::Frame(next);
    }

    fn jump_to(&mut self, idx: Option<usize>) {
        if self.frames.is_empty() {
            return;
        }
        let last = self.frames.len() - 1;
        let target = idx.unwrap_or(last).min(last);
        self.viewer = ViewerState::Frame(target);
    }

    fn jump_to_end(&mut self) {
        if self.frames.is_empty() {
            return;
        }
        self.viewer = ViewerState::Frame(self.frames.len() - 1);
    }

    fn toggle_play(&mut self) {
        if self.frames.is_empty() {
            return;
        }
        self.viewer = match self.viewer {
            ViewerState::Playing { .. } => {
                // Pause on the current frame.
                let i = self.viewer.frame_index().unwrap_or(0);
                ViewerState::Frame(i)
            }
            ViewerState::Live => ViewerState::Playing { index: 0, last_advance: Instant::now() },
            ViewerState::Frame(i) => {
                let start = if i == self.frames.len() - 1 { 0 } else { i };
                ViewerState::Playing { index: start, last_advance: Instant::now() }
            }
        };
        self.playback_tick = Instant::now();
    }

    /// Advance the playback cursor by one frame if the fps interval has passed.
    fn tick_playback(&mut self) {
        let ViewerState::Playing { index, last_advance } = self.viewer else { return };
        if self.frames.is_empty() {
            self.viewer = ViewerState::Live;
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
            self.viewer = ViewerState::Frame(self.frames.len() - 1);
        } else {
            self.viewer = ViewerState::Playing {
                index: next,
                last_advance: last_advance + step * (steps as u32),
            };
        }
        self.playback_tick = Instant::now();
    }

    // ---- ui -------------------------------------------------------------

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("DragonSlayer");
            ui.label(RichText::new(concat!("v", env!("CARGO_PKG_VERSION"))).small().color(ui.visuals().weak_text_color()));
            if let Some(p) = &self.project {
                ui.separator();
                ui.label(RichText::new(p.name()).strong());
            }
            ui.separator();
            if ui.button("New project…").clicked() {
                self.new_project_dialog();
            }
            if ui.button("Open…").clicked()
                && let Some(dir) = rfd::FileDialog::new().set_title("Open a DragonSlayer project folder").pick_folder() {
                    self.open_project(&dir);
                }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("Help (H)").on_hover_text("Everything you need to shoot — or press H").clicked() {
                    log_line(&format!("Help button clicked; help_open was {}", self.help_open));
                    self.help_open = true;
                }
                ui.separator();
                self.camera_status(ui);
            });
        });
    }

    fn camera_status(&mut self, ui: &mut egui::Ui) {
        // Actionable driver state gets its own row with a Set up button.
        if let Status::WrongDriver { name } = &self.status {
            let n = name.clone().unwrap_or_else(|| "Camera".into());
            if ui
                .button("Set up USB driver…")
                .on_hover_text("One-time driver swap so DragonSlayer can talk to the camera")
                .clicked()
            {
                launch_driver_setup();
            }
            ui.label(RichText::new(format!("{n} · needs driver setup")).color(Color32::from_rgb(230, 160, 60)));
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
            ui.painter().circle_filled(rect.center(), 5.0, Color32::from_rgb(230, 160, 60));
            return;
        }
        let (dot, text, detail) = match &self.status {
            Status::Connected { name, caps } => (
                Color32::from_rgb(60, 180, 90),
                format!("{name} · connected"),
                (!caps.live_view).then(|| "No live view on this model: onion skin shows over the last frame.".to_string()),
            ),
            Status::Searching => (
                Color32::from_rgb(200, 160, 40),
                "No camera".into(),
                Some(connect_hint().to_string()),
            ),
            Status::NoBackend => (
                Color32::GRAY,
                "No camera support in this build".into(),
                Some("Rebuild with `--features gphoto2`, or start with --mock to try the mock camera.".into()),
            ),
            Status::Problem { name, message } => (
                Color32::from_rgb(220, 70, 60),
                name.clone().unwrap_or_else(|| "Camera".into()) + " · problem",
                Some(message.clone()),
            ),
            Status::WrongDriver { .. } => unreachable!(),
        };
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

    fn controls(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let can_capture = self.camera_ready() && !self.capturing && self.active_row().is_some();
            let label = if self.capturing { "Capturing…" } else { "Capture" };
            let capture = egui::Button::new(RichText::new(label).size(20.0).strong())
                .fill(Color32::from_rgb(190, 50, 45))
                .min_size(Vec2::new(150.0, 40.0));
            if ui.add_enabled(can_capture, capture).on_hover_text("Space").clicked() {
                self.capture();
            }
            let has_frames = self.active_row().is_some_and(|r| r.count > 0);
            if ui
                .add_enabled(has_frames, egui::Button::new("Delete last").min_size(Vec2::new(0.0, 40.0)))
                .on_hover_text("Backspace. The frame moves to the scene's trash folder.")
                .clicked()
            {
                self.delete_last();
            }

            ui.separator();
            ui.checkbox(&mut self.onion_on, "Onion skin").on_hover_text("O");
            ui.add_enabled_ui(self.onion_on, |ui| {
                ui.add(egui::Slider::new(&mut self.onion_count, 1..=5).text("frames"));
                ui.add(egui::Slider::new(&mut self.onion_opacity, 0.05..=0.9).text("opacity").show_value(false));
                ui.checkbox(&mut self.onion_edges, "Outlines")
                    .on_hover_text("Show only edges of previous frames instead of a full ghost image");
            });

            ui.separator();
            // Transport controls: jump-to-start / step back / play|pause / step forward / jump-to-end,
            // and the current frame position.
            let count = self.frames.len();
            let idx = self.viewer.frame_index();
            let playing = self.viewer.is_playing();
            let live = self.viewer.is_live();
            let has_frames = count > 0;
            ui.add_enabled_ui(has_frames, |ui| {
                if ui.button("⏮").on_hover_text("Home — first frame").clicked() {
                    self.jump_to(Some(0));
                }
                if ui.button("◀").on_hover_text("Left arrow — previous frame (Shift for 10)").clicked() {
                    self.step(-1);
                }
                let play_label = if playing { "⏸" } else { "▶" };
                if ui.button(play_label).on_hover_text("P — play/pause at scene fps").clicked() {
                    self.toggle_play();
                }
                if ui.button("▶|").on_hover_text("Right arrow — next frame (Shift for 10)").clicked() {
                    self.step(1);
                }
                if ui.button("⏭").on_hover_text("End — last frame").clicked() {
                    self.jump_to_end();
                }
            });
            ui.separator();
            // Position readout, and a Live button so users can get back.
            let position = if live {
                "LIVE".to_string()
            } else if playing {
                format!("Playing {}/{}", idx.map(|i| i + 1).unwrap_or(0), count.max(1))
            } else if let Some(i) = idx {
                format!("Frame {}/{}", i + 1, count.max(1))
            } else {
                format!("— / {}", count)
            };
            ui.label(RichText::new(position).monospace());
            let live_ok = self.has_live_view();
            ui.add_enabled_ui(live_ok, |ui| {
                if ui.selectable_label(live, "Live").on_hover_text("Esc").clicked() {
                    self.viewer = ViewerState::Live;
                }
            });

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("Compile…").clicked() {
                    self.compile.open = true;
                    self.compile.result = None;
                }
            });
        });

        ui.horizontal(|ui| {
            if let Some((msg, is_err, at)) = &self.message
                && (at.elapsed() < Duration::from_secs(8) || *is_err) {
                    let color = if *is_err { Color32::from_rgb(220, 80, 70) } else { ui.visuals().weak_text_color() };
                    ui.label(RichText::new(msg).color(color));
                }
        });
        ui.add_space(4.0);
    }

    fn viewer(&mut self, ui: &mut egui::Ui) {
        let area = ui.available_rect_before_wrap();
        ui.painter().rect_filled(area, 0.0, Color32::from_gray(18));

        let width = self.onion_width();
        let n = self.frames.len();

        // Base image and where onion frames should end (exclusive) depend on viewer state.
        let (base, onion_end, mode_label) = match self.viewer {
            ViewerState::Live => {
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
            ViewerState::Frame(i) => {
                let base = self.frames.get(i).and_then(|f| f.jpeg()).and_then(|p| self.images.get(p, width));
                (base, i, format!("FRAME {}/{}", i + 1, n.max(1)))
            }
            ViewerState::Playing { index, .. } => {
                let base = self.frames.get(index).and_then(|f| f.jpeg()).and_then(|p| self.images.get(p, width));
                (base, index, format!("PLAY {}/{}", index + 1, n.max(1)))
            }
        };
        let showing_live = self.viewer.is_live() && self.live.is_some();

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

        let rect = fit(area.shrink(8.0), base.size_vec2());
        let uv = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
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

        // Picture-in-picture live view: when reviewing a captured frame or playing back,
        // keep the camera's live feed visible in the corner so the animator can line up
        // the next shot while looking at the last one.
        if !self.viewer.is_live()
            && let Some(live) = &self.live
        {
            let pip_w = (area.width() * 0.22).clamp(180.0, 320.0);
            let live_size = live.size_vec2();
            let aspect = if live_size.x > 0.0 { live_size.y / live_size.x } else { 0.5625 };
            let pip_h = pip_w * aspect;
            let margin = 12.0;
            let pip_rect = Rect::from_min_size(
                egui::pos2(rect.right() - pip_w - margin, rect.top() + margin),
                Vec2::new(pip_w, pip_h),
            );
            painter.rect_filled(pip_rect.expand(4.0), 4.0, Color32::from_black_alpha(180));
            painter.image(
                live.id(),
                pip_rect,
                Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                Color32::WHITE,
            );
            painter.rect_stroke(
                pip_rect,
                3.0,
                Stroke::new(1.5, Color32::from_rgb(230, 90, 90)),
                egui::epaint::StrokeKind::Outside,
            );
            let dot = pip_rect.left_top() + Vec2::new(10.0, 10.0);
            painter.circle_filled(dot, 4.0, Color32::from_rgb(230, 60, 60));
            painter.text(
                dot + Vec2::new(10.0, -2.0),
                Align2::LEFT_TOP,
                "LIVE",
                FontId::proportional(12.0),
                Color32::WHITE,
            );
            painter.text(
                pip_rect.right_bottom() + Vec2::new(-6.0, -4.0),
                Align2::RIGHT_BOTTOM,
                "click to swap",
                FontId::proportional(10.0),
                Color32::from_white_alpha(180),
            );
            // Click the PIP to swap the main viewer back to live.
            let resp = ui.interact(pip_rect, egui::Id::new("live pip"), Sense::click());
            if resp.clicked() {
                self.viewer = ViewerState::Live;
            }
            if resp.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
        }

        // Also let a click on the main viewer, when we're reviewing, offer a way to jump
        // back to live. Right-click anywhere in the viewer → back to live view.
        if !self.viewer.is_live() {
            let resp = ui.interact(rect, egui::Id::new("viewer main"), Sense::click());
            if resp.secondary_clicked() {
                self.viewer = ViewerState::Live;
            }
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
        let cur = self.viewer.frame_index().unwrap_or(n.saturating_sub(1));
        let half = visible / 2;
        let start = cur.saturating_sub(half).min(n.saturating_sub(visible.min(n)));
        let end = (start + visible).min(n);

        for (draw_i, i) in (start..end).enumerate() {
            let x = rect.left() + 4.0 + draw_i as f32 * stride;
            let r = Rect::from_min_size(egui::pos2(x, rect.top() + 5.0), Vec2::new(thumb_w, thumb_h));
            let is_current = i == cur && !self.viewer.is_live();
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
        if resp.clicked() || resp.dragged() {
            if let Some(p) = resp.interact_pointer_pos() {
                let x = p.x - rect.left() - 4.0;
                if x >= 0.0 {
                    let draw_i = (x / stride).floor() as usize;
                    let target = (start + draw_i).min(n - 1);
                    self.viewer = ViewerState::Frame(target);
                }
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
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Compiling…");
                    });
                } else if ui.button(RichText::new("Compile").strong()).clicked()
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
                        thread::spawn(move || {
                            let r = compile::compile(&p, scene.as_deref(), &settings).map_err(|e| e.to_string());
                            let _ = tx.send(r);
                            ctx.request_repaint();
                        });
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
                ui.label(RichText::new("Instructions for:").small());
                ui.selectable_value(&mut self.help_os, HelpOs::Windows, "Windows");
                ui.selectable_value(&mut self.help_os, HelpOs::Mac, "macOS");
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("Close").clicked() {
                        self.help_open = false;
                    }
                });
            });
            ui.separator();
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                help_content(ui, self.help_os);
            });
        });
        // Click outside the modal or press Escape → close.
        if response.should_close() {
            self.help_open = false;
        }
    }
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
            ui.label(RichText::new(format!(" {key} ")).monospace().background_color(Color32::from_gray(50)));
            ui.label(what);
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
    k(ui, "Space", "Capture the next frame into the active scene");
    k(ui, "Backspace", "Move the last frame to that scene's trash folder (undo delete by \
        moving it back from disk — nothing is destroyed)");
    k(ui, "O", "Toggle onion skin on/off");
    k(ui, "Tab", "Switch between live view and the last captured frame");

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

    h(ui, "Compile");
    p(ui, "Compile… asks whether to render this scene or the whole project, what format \
        (H.264 MP4 or ProRes 422 MOV), what resolution (source, 4K, 1080p) and how to frame \
        (fit with black bars, or crop to fill). Videos land in your project's exports/ folder \
        and are never overwritten.");

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
    p(ui, "· Canon EOS: usually PC / PTP by default when connected to a computer.");
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
    p(ui, "2. If still nothing: unplug the USB, wait 3 seconds, plug it back in (a different \
        port on the computer is fine and sometimes helps).");
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

/// Simple debug log to %TEMP%\dragonslayer.log so we can see what an off-machine build actually does.
fn log_line(msg: &str) {
    use std::io::Write;
    let Ok(mut dir) = std::env::var("TEMP").map(std::path::PathBuf::from) else { return };
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

impl eframe::App for DragonSlayerApp {
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw: &mut egui::RawInput) {
        self.take_shortcuts(raw);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.images.begin_frame(&ctx);
        self.handle_events(&ctx);
        self.handle_keys();

        egui::Panel::top("top").show(ui, |ui| {
            ui.add_space(4.0);
            self.top_bar(ui);
            ui.add_space(4.0);
        });

        if self.project.is_none() {
            egui::CentralPanel::default().show(ui, |ui| self.welcome(ui));
            // Still let modals render on the welcome screen (Help is the important one).
            self.help_window(&ctx);
            return;
        }

        egui::Panel::left("scenes").resizable(true).default_size(240.0).min_size(180.0).show(ui, |ui| {
            self.scene_list(ui);
        });
        egui::Panel::bottom("controls").show(ui, |ui| self.controls(ui));
        egui::Panel::bottom("timeline").show(ui, |ui| self.timeline(ui));
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| self.viewer(ui));

        self.compile_window(&ctx);
        self.help_window(&ctx);

        // Advance playback if we're currently playing.
        self.tick_playback();

        // Keep live view, capture spinner, compile progress and playback repainting smoothly.
        if self.live.is_some() || self.capturing || self.compile.running.is_some() || self.viewer.is_playing() {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
    }
}
