//! Menu bar and the status bar under it.

use super::*;

impl DragonSlayerApp {
    /// Classic app menu bar: File / Edit / View / Scene / Compile / Help.
    pub(super) fn menu_bar(&mut self, ui: &mut egui::Ui) {
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
                let mut open_recent: Option<PathBuf> = None;
                ui.menu_button(format!("{}  Open recent", ph::CLOCK_COUNTER_CLOCKWISE), |ui| {
                    let current = self.project.as_ref().map(|p| p.root.clone());
                    let mut any = false;
                    for r in self.welcome_recent.iter().filter(|r| r.found()).take(20) {
                        if current.as_deref() == Some(r.path.as_path()) {
                            continue;
                        }
                        any = true;
                        let text = format!("{}  ·  {} frames", r.name.as_deref().unwrap_or("?"), r.frames);
                        if ui.button(text).on_hover_text(r.path.display().to_string()).clicked() {
                            open_recent = Some(r.path.clone());
                            ui.close();
                        }
                    }
                    if !any {
                        ui.label(RichText::new("No other projects yet").color(crate::theme::palette().text_muted));
                    }
                    ui.separator();
                    if ui.button(format!("{}  Find my projects", ph::MAGNIFYING_GLASS)).clicked() {
                        let ctx = ui.ctx().clone();
                        self.start_project_scan(&ctx);
                        ui.close();
                    }
                });
                if let Some(path) = open_recent {
                    self.open_project(&path);
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
                    let n = self.scene_count() + 1;
                    let after = self.active_row().map(|r| r.id.clone());
                    self.edit(|p| p.add_scene(&format!("Scene {n}"), after.as_deref()).map(|_| ()));
                    ui.close();
                }
                if ui
                    .add_enabled(self.active_row().is_some(), egui::Button::new(format!("{}  New take of this scene", ph::COPY_SIMPLE)))
                    .on_hover_text("Shoot the scene again without losing what you have; right-click a take to use it in the film")
                    .clicked()
                {
                    if let Some(id) = self.active_row().map(|r| r.id.clone()) {
                        self.new_take(&id);
                    }
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
                    self.open_compile(None);
                    ui.close();
                }
                if ui
                    .add_enabled(self.project.is_some(), egui::Button::new(format!("{}  Compile for edit…", ph::FOLDER)))
                    .on_hover_text("One ProRes file per scene, named after the scene, in its own folder")
                    .clicked()
                {
                    self.open_compile(Some(Scope::Each));
                    ui.close();
                }
            });
            ui.menu_button(format!("{}  Tools", ph::WRENCH), |ui| {
                if ui
                    .add_enabled(self.project.is_some(), egui::Button::new(format!("{}  Import images…", ph::DOWNLOAD_SIMPLE)))
                    .on_hover_text("Pull photos from a camera card or folder into a scene, e.g. to rescue a lost project")
                    .on_disabled_hover_text("Open or create a project first")
                    .clicked()
                {
                    ui.close();
                    self.start_import(ui.ctx());
                }
                ui.separator();
                if ui.button(format!("{}  Diagnose camera…", ph::STETHOSCOPE)).clicked() {
                    self.diagnose_open = true;
                    self.start_usb_scan(ui.ctx());
                    ui.close();
                }
                if cfg!(windows)
                    && ui
                        .button(format!("{}  Set up USB driver (Zadig)…", ph::USB))
                        .on_hover_text("One-time driver swap for the USB port the camera is plugged into")
                        .clicked()
                {
                    launch_driver_setup();
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
    pub(super) fn status_bar(&mut self, ui: &mut egui::Ui) {
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
}
