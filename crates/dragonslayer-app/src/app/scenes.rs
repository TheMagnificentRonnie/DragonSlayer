//! The scene list: select, rename, reorder, per-scene menu.

use super::*;

impl DragonSlayerApp {
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
}

enum SceneAction {
    Activate(String),
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
        if active { text_color.gamma_multiply(0.8) } else { visuals.weak_text_color() },
    );
    resp
}
