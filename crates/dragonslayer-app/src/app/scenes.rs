//! The scene list: select, rename, reorder, per-scene menu.

use super::*;

impl DragonSlayerApp {
    /// Starts a new take of the scene (or of the scene a take belongs to); it becomes the
    /// active scene, so Space shoots into it.
    pub(super) fn new_take(&mut self, id: &str) {
        let id = id.to_owned();
        let mut made = None;
        self.edit(|p| p.add_take(&id).map(|s| made = Some(s.name().to_owned())));
        if let Some(name) = made {
            self.mode = Mode::Capture;
            self.info(format!("New take: {name}. Captures go into it; the film keeps the take marked ★."));
        }
    }

    pub(super) fn scene_list(&mut self, ui: &mut egui::Ui) {
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
                // Takes move with their scene: only scenes can be dragged.
                if resp.drag_started() && row.owner.is_none() {
                    self.drag_from = Some(i);
                }
                resp.context_menu(|ui| {
                    if ui.button("Rename").clicked() {
                        self.renaming = Some((row.id.clone(), row.name.clone()));
                        ui.close();
                    }
                    if ui
                        .button("New take")
                        .on_hover_text("Shoot this scene again without losing what you have; choose which take the film uses")
                        .clicked()
                    {
                        action = Some(SceneAction::NewTake(row.id.clone()));
                        ui.close();
                    }
                    let scene_id = row.owner.clone().unwrap_or_else(|| row.id.clone());
                    let has_takes = row.owner.is_some() || row.takes > 1;
                    if has_takes && !row.in_film && ui.button("Use this take in the film").clicked() {
                        let take = row.owner.is_some().then(|| row.id.clone());
                        action = Some(SceneAction::UseTake(scene_id, take));
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
                    let delete = match (&row.owner, row.takes) {
                        (Some(_), _) => "Delete take (moves to trash)",
                        (None, n) if n > 1 => "Delete scene and its takes (moves to trash)",
                        _ => "Delete scene (moves to trash)",
                    };
                    if ui.button(delete).clicked() {
                        action = Some(SceneAction::Delete(row.id.clone()));
                        ui.close();
                    }
                });
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
                    // Rows include takes; the project order counts scenes only.
                    let scenes_before = |row: usize| self.scenes[..row.min(self.scenes.len())].iter().filter(|r| r.owner.is_none()).count();
                    let (from_s, t_s) = (scenes_before(from), scenes_before(t));
                    let to = if t_s > from_s { t_s - 1 } else { t_s };
                    if to != from_s {
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
            Some(SceneAction::Rename(id, name)) => self.edit(|p| p.rename_scene(&id, &name)),
            Some(SceneAction::Fps(id, fps)) => self.edit(|p| p.set_scene_fps(&id, fps)),
            Some(SceneAction::Move(id, to)) => self.edit(|p| p.move_scene(&id, to)),
            Some(SceneAction::Delete(id)) => {
                self.edit(|p| p.delete_scene(&id));
                self.info("Moved to the project's trash folder");
            }
            Some(SceneAction::NewTake(id)) => self.new_take(&id),
            Some(SceneAction::UseTake(scene, take)) => {
                self.edit(|p| p.choose_take(&scene, take.as_deref()));
                self.info("The film now uses this take");
            }
            None => {}
        }
    }
}

enum SceneAction {
    Activate(String),
    NewTake(String),
    /// (scene, take): `None` is take 1, the scene itself.
    UseTake(String, Option<String>),
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
    // Takes sit indented under their scene, a little shorter.
    let (indent, height) = if row.owner.is_some() { (22.0, 46.0) } else { (0.0, 58.0) };
    let (full, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::click_and_drag());
    let rect = Rect::from_min_max(full.min + Vec2::new(indent, 0.0), full.max);
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
            // A take is shown as "Take 2" under its scene; the full name is on hover. With
            // more than one take, a star marks the one the film uses.
            let mut title = if row.owner.is_some() { format!("Take {}", row.take) } else { row.name.clone() };
            if (row.owner.is_some() || row.takes > 1) && row.in_film {
                title.push_str("  ★");
            }
            ui.painter().text(
                egui::pos2(text_x, rect.top() + if row.owner.is_some() { 6.0 } else { 10.0 }),
                Align2::LEFT_TOP,
                title,
                FontId::proportional(if row.owner.is_some() { 14.0 } else { 15.0 }),
                text_color,
            );
        }
    }
    let fps = row.fps.map(|f| format!(" · {f} fps")).unwrap_or_default();
    let mut detail = format!("{} frames{fps}", row.count);
    if row.owner.is_none() && row.takes > 1 {
        detail.push_str(&format!(" · {} takes", row.takes));
    }
    ui.painter().text(
        egui::pos2(text_x, rect.top() + if row.owner.is_some() { 26.0 } else { 32.0 }),
        Align2::LEFT_TOP,
        detail,
        FontId::proportional(12.0),
        if active { text_color.gamma_multiply(0.8) } else { visuals.weak_text_color() },
    );
    // Screen readers (and the UI tests) get the name shown on the row.
    let title = if row.owner.is_some() { format!("Take {}", row.take) } else { row.name.clone() };
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, active, &title));
    let in_film = if (row.owner.is_some() || row.takes > 1) && row.in_film { "\n★ This take is in the film" } else { "" };
    resp.on_hover_text(format!("{}{in_film}", row.name))
}
