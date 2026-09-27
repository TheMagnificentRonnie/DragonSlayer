//! Welcome screen: new project, open, continue where you left off.

use super::*;

impl DragonSlayerApp {
    pub(super) fn new_project_dialog(&mut self) {
        let mut dialog = rfd::FileDialog::new()
            .set_title("New project: name your film (it becomes a folder)")
            .set_file_name("My Film");
        // One predictable home for projects, so they're easy to find again.
        if let Some(dir) = crate::recent::default_projects_dir()
            && std::fs::create_dir_all(&dir).is_ok()
        {
            dialog = dialog.set_directory(dir);
        }
        let Some(path) = dialog.save_file() else {
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

    pub(super) fn welcome(&mut self, ui: &mut egui::Ui) {
        let pal = crate::theme::palette();
        let mut open: Option<PathBuf> = None;
        let mut new_project = false;
        let mut find = false;
        let mut forget: Option<PathBuf> = None;
        let has_resume = self.welcome_recent.first().is_some_and(RecentProject::found);
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * if has_resume { 0.15 } else { 0.3 });
            ui.heading("Make a stop-motion film with your camera");
            ui.add_space(14.0);

            if let Some(r) = self.welcome_recent.first().filter(|r| r.found()) {
                if self.last_session_crashed {
                    ui.label(
                        RichText::new(
                            "DragonSlayer didn't close properly last time (a crash, a forced close or a power cut).\n\
                             Your frames are safe: anything caught mid-shot is recovered when you continue.",
                        )
                        .color(pal.warn),
                    );
                    ui.add_space(8.0);
                }
                egui::Frame::group(ui.style()).inner_margin(Margin::same(12)).show(ui, |ui| {
                    ui.set_width(420.0);
                    ui.horizontal(|ui| {
                        let (thumb, _) = ui.allocate_exact_size(Vec2::new(128.0, 80.0), Sense::hover());
                        ui.painter().rect_filled(thumb, 3.0, Color32::from_gray(30));
                        if let Some(tex) = r.last_jpeg.as_ref().and_then(|p| self.images.get(p, THUMB_WIDTH)) {
                            let rect = fit(thumb, tex.size_vec2());
                            let uv = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
                            ui.painter().image(tex.id(), rect, uv, Color32::WHITE);
                        }
                        ui.vertical(|ui| {
                            ui.label(RichText::new("You were working on").color(pal.text_muted));
                            ui.label(RichText::new(r.name.as_deref().unwrap_or("a project")).size(18.0).strong());
                            if let Some(scene) = &r.scene {
                                ui.label(format!("{scene} · {} frames", r.frames));
                            }
                            ui.add_space(4.0);
                            let go = egui::Button::new(
                                RichText::new("Continue where you left off").strong().color(pal.on_accent),
                            )
                            .fill(pal.accent);
                            if ui.add(go).clicked() {
                                open = Some(r.path.clone());
                            }
                        });
                    });
                });
                ui.add_space(14.0);
            }

            if ui.add(egui::Button::new(RichText::new("New project").size(18.0))).clicked() {
                new_project = true;
            }
            ui.add_space(6.0);
            if ui.button("Open a project folder…").clicked()
                && let Some(dir) = rfd::FileDialog::new().pick_folder()
            {
                open = Some(dir);
            }

            ui.add_space(18.0);
            ui.horizontal(|ui| {
                let w = 520.0;
                ui.add_space(((ui.available_width() - w) / 2.0).max(0.0));
                ui.label(RichText::new("Your projects").strong());
                if self.project_scan.is_some() {
                    ui.spinner();
                    ui.label(RichText::new("looking for more…").small().color(pal.text_muted));
                } else if ui
                    .small_button("Find my projects")
                    .on_hover_text("Search your folders and drives for DragonSlayer projects")
                    .clicked()
                {
                    find = true;
                }
            });
            let others = &self.welcome_recent[usize::from(has_resume).min(self.welcome_recent.len())..];
            if others.is_empty() && self.project_scan.is_none() {
                ui.label(RichText::new("None yet.").color(pal.text_muted));
            }
            // Centred like the rest of the screen; each project is one clickable card.
            let list_w = 540.0_f32.min(ui.available_width());
            ui.allocate_ui_with_layout(Vec2::new(list_w, 280.0), Layout::top_down(Align::Min), |ui| {
                egui::ScrollArea::vertical().max_height(280.0).show(ui, |ui| {
                    for r in others {
                        match &r.name {
                            Some(name) => {
                                if project_card(ui, name, r.frames, &r.path, list_w).clicked() {
                                    open = Some(r.path.clone());
                                }
                            }
                            None => {
                                ui.horizontal_wrapped(|ui| {
                                    ui.label(RichText::new(r.path.display().to_string()).small().color(pal.text_muted));
                                    ui.label(RichText::new("not found: moved, or on a drive that isn't plugged in").small().color(pal.warn));
                                    if ui.small_button("Forget").on_hover_text("Remove from this list. Doesn't delete anything.").clicked() {
                                        forget = Some(r.path.clone());
                                    }
                                });
                            }
                        }
                        ui.add_space(4.0);
                    }
                });
            });
        });
        if find {
            let ctx = ui.ctx().clone();
            self.start_project_scan(&ctx);
        }
        if let Some(p) = forget {
            self.recent.forget(&p);
            self.rebuild_projects_list();
        }
        if new_project {
            self.new_project_dialog();
        }
        if let Some(path) = open {
            self.open_project(&path);
        }
    }
}

impl DragonSlayerApp {
    /// Searches folders and drives for projects in the background; found ones join the list.
    pub(super) fn start_project_scan(&mut self, ctx: &egui::Context) {
        if self.project_scan.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        thread::spawn(move || {
            let found = crate::recent::find_projects(&crate::recent::search_roots(), Duration::from_secs(30));
            let _ = tx.send(found);
            ctx.request_repaint();
        });
        self.project_scan = Some(rx);
    }

    pub(super) fn poll_project_scan(&mut self) {
        let Some(rx) = &self.project_scan else { return };
        let Ok(found) = rx.try_recv() else { return };
        self.project_scan = None;
        let added = self.recent.add_found(&found);
        if added > 0 {
            log_line(&format!("found {added} projects not in the list"));
            self.rebuild_projects_list();
        }
    }

    pub(super) fn rebuild_projects_list(&mut self) {
        self.welcome_recent = self.recent.projects.iter().map(|p| RecentProject::describe(p)).collect();
    }
}

/// One project in the welcome list: the whole card is the button. Name and frame count on
/// top, the folder underneath; highlights on hover with a hand cursor.
fn project_card(ui: &mut egui::Ui, name: &str, frames: usize, path: &Path, width: f32) -> egui::Response {
    let pal = crate::theme::palette();
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 46.0), Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("Open this project");
    let (fill, stroke) = if resp.hovered() { (pal.bg_hover, pal.accent) } else { (pal.bg_elevated, pal.border) };
    let p = ui.painter();
    p.rect(rect, 6.0, fill, Stroke::new(1.0, stroke), egui::epaint::StrokeKind::Inside);
    let left = rect.left() + 12.0;
    p.text(egui::pos2(left, rect.top() + 7.0), Align2::LEFT_TOP, name, FontId::proportional(15.0), pal.text_primary);
    p.text(
        egui::pos2(rect.right() - 12.0, rect.top() + 8.0),
        Align2::RIGHT_TOP,
        format!("{frames} frames"),
        FontId::proportional(12.5),
        pal.text_muted,
    );
    p.text(
        egui::pos2(left, rect.bottom() - 7.0),
        Align2::LEFT_BOTTOM,
        path.display().to_string(),
        FontId::proportional(11.5),
        pal.text_muted,
    );
    // Screen readers (and the UI tests) see it as a button with the project's name.
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, name));
    resp
}

impl RecentProject {
    pub(super) fn describe(path: &Path) -> Self {
        let mut r = Self { path: path.to_owned(), name: None, scene: None, frames: 0, last_jpeg: None };
        if let Ok(p) = Project::open(path) {
            r.name = Some(p.name().to_owned());
            if let Ok(s) = p.active_scene() {
                r.scene = Some(s.name().to_owned());
                if let Ok(frames) = s.frames() {
                    r.frames = frames.len();
                    r.last_jpeg = frames.last().and_then(|f| f.jpeg().map(Path::to_path_buf));
                }
            }
        }
        r
    }

    pub(super) fn found(&self) -> bool {
        self.name.is_some()
    }
}
