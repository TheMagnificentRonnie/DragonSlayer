//! Bodies of the dockable panels, and the egui_dock glue.

use super::*;

impl DragonSlayerApp {
    pub(super) fn tab_scenes(&mut self, ui: &mut egui::Ui) {
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

    pub(super) fn tab_camera(&mut self, ui: &mut egui::Ui) {
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

    pub(super) fn tab_exposure(&mut self, ui: &mut egui::Ui) {
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
                                let text = if s.kind == dragonslayer_camera::SettingKind::ImageFormat && is_raw_only(c) {
                                    format!("{c}  (no JPEG: can't preview or compile)")
                                } else {
                                    c.clone()
                                };
                                ui.selectable_value(&mut selected, c.clone(), text);
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

        let raw_only = self
            .camera_settings
            .iter()
            .any(|s| s.kind == dragonslayer_camera::SettingKind::ImageFormat && is_raw_only(&s.value));
        if raw_only {
            ui.add_space(8.0);
            ui.label(
                RichText::new(
                    "Image format is RAW only. DragonSlayer shows, onion-skins and compiles the JPEG, so new \
                     frames won't appear. Choose a RAW + JPEG option to keep both.",
                )
                .color(crate::theme::palette().warn),
            );
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

    pub(super) fn tab_onion(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        ui.checkbox(&mut self.onion_on, "Show onion skin (O)");
        ui.add_enabled_ui(self.onion_on, |ui| {
            ui.add(egui::Slider::new(&mut self.onion_count, 1..=5).text("frames"));
            ui.add(egui::Slider::new(&mut self.onion_opacity, 0.05..=0.9).text("opacity"));
            ui.checkbox(&mut self.onion_edges, "Outlines only (crisper over live view)");
        });
    }

    pub(super) fn tab_export(&mut self, ui: &mut egui::Ui) {
        use egui_phosphor::regular as ph;
        ui.add_space(4.0);
        if ui
            .add(
                egui::Button::new(format!("{}  Compile video…", ph::EXPORT))
                    .min_size(Vec2::new(ui.available_width(), 36.0)),
            )
            .clicked()
        {
            self.open_compile(None);
        }
        if ui
            .add(
                egui::Button::new(format!("{}  Compile for edit…", ph::FOLDER))
                    .min_size(Vec2::new(ui.available_width(), 30.0)),
            )
            .on_hover_text("One ProRes file per scene, named after the scene, in its own folder")
            .clicked()
        {
            self.open_compile(Some(Scope::Each));
        }
        ui.add_space(6.0);
        ui.label(
            RichText::new("Renders your scenes to MP4 or ProRes MOV using ffmpeg. Files land in exports/ inside the project folder and are never overwritten.")
                .small()
                .color(crate::theme::palette().text_muted),
        );
    }

    pub(super) fn tab_timeline(&mut self, ui: &mut egui::Ui) {
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
                ui.separator();
                if ui
                    .selectable_label(self.mark_in.is_some(), "[ In")
                    .on_hover_text("[ — mark the start of a range at this frame (again to clear)")
                    .clicked()
                {
                    self.toggle_mark_in();
                }
                if ui
                    .selectable_label(self.mark_out.is_some(), "Out ]")
                    .on_hover_text("] — mark the end of a range at this frame (again to clear)")
                    .clicked()
                {
                    self.toggle_mark_out();
                }
                if ui
                    .selectable_label(self.loop_on, format!("{}  Loop", ph::REPEAT))
                    .on_hover_text("L — keep playing the marked range (or the whole scene) round and round")
                    .clicked()
                {
                    self.loop_on = !self.loop_on;
                }
                if let Some((a, b)) = self.marked_range() {
                    ui.label(
                        RichText::new(format!("{}–{} ({} frames)", a + 1, b + 1, b - a + 1))
                            .monospace()
                            .color(crate::theme::palette().accent),
                    );
                    if ui.small_button("Clear").on_hover_text("Remove the in and out marks").clicked() {
                        self.clear_marks();
                    }
                }
            }
        });
        self.timeline(ui);
    }
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

/// An image format that produces no JPEG: "RAW" on Canon and Panasonic, but not
/// "RAW + Large Fine JPEG" or "RAW+Fine".
pub(super) fn is_raw_only(format: &str) -> bool {
    let f = format.to_ascii_uppercase();
    f.contains("RAW") && !f.contains('+') && !f.contains("JPEG") && !f.contains("JPG")
}
