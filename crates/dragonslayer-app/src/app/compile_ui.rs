//! The Compile video window.

use super::*;

impl DragonSlayerApp {
    /// Opens the Compile window, optionally preset to a scope. Picking a scene defaults
    /// to the active one; compile-for-edit defaults to ProRes.
    pub(super) fn open_compile(&mut self, scope: Option<Scope>) {
        let active = self.active_row().map(|r| r.id.clone());
        let d = &mut self.compile;
        d.open = true;
        d.result = None;
        if let Some(scope) = scope {
            if scope == Scope::Each && d.scope != Scope::Each {
                d.format = Format::ProRes;
            }
            d.scope = scope;
        }
        let known = |id: &String| self.scenes.iter().any(|r| &r.id == id);
        if !d.scene.as_ref().is_some_and(known) {
            d.scene = active;
        }
    }

    pub(super) fn compile_window(&mut self, ctx: &egui::Context) {
        if !self.compile.open {
            return;
        }
        let mut open = true;
        let scenes: Vec<(String, String, usize)> = self.scenes.iter().map(|r| (r.id.clone(), r.name.clone(), r.count)).collect();
        // Marks live on the active scene, so "only the marked frames" applies to that one.
        let marked = self.marked_range();
        let active_id = self.active_row().map(|r| r.id.clone());
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
                        let scene_label = |id: &Option<String>| {
                            scenes
                                .iter()
                                .find(|(sid, ..)| Some(sid) == id.as_ref())
                                .map(|(_, name, n)| format!("Scene: {name} ({n} frames)"))
                                .unwrap_or_else(|| "Scene".into())
                        };
                        let selected = match d.scope {
                            Scope::Project => "Whole project (one video)".to_string(),
                            Scope::Each => "Each scene, for editing (one file per scene)".to_string(),
                            Scope::Scene => scene_label(&d.scene),
                        };
                        let before = d.scope;
                        egui::ComboBox::from_id_salt("compile what").selected_text(selected).width(320.0).show_ui(ui, |ui| {
                            ui.selectable_value(&mut d.scope, Scope::Project, "Whole project (one video)");
                            ui.selectable_value(&mut d.scope, Scope::Each, "Each scene, for editing (one file per scene)");
                            ui.separator();
                            for (id, name, n) in &scenes {
                                let on = d.scope == Scope::Scene && d.scene.as_ref() == Some(id);
                                if ui.selectable_label(on, format!("Scene: {name} ({n} frames)")).clicked() {
                                    d.scope = Scope::Scene;
                                    d.scene = Some(id.clone());
                                }
                            }
                        });
                        if d.scope == Scope::Each && before != Scope::Each {
                            d.format = Format::ProRes;
                        }
                        ui.end_row();
                        let marks_apply = d.scope == Scope::Scene && d.scene.is_some() && d.scene == active_id;
                        if let (true, Some((a, b))) = (marks_apply, marked) {
                            ui.label("Frames");
                            ui.checkbox(&mut d.only_marked, format!("Only the marked frames ({}–{})", a + 1, b + 1))
                                .on_hover_text("The range set with [ and ] on the timeline");
                            ui.end_row();
                        }
                        if d.scope == Scope::Each {
                            ui.label("");
                            ui.label(
                                RichText::new("A new folder in exports/ with one file per scene, named after it and numbered in film order (01 Opening.mov, 02 …). Empty scenes are skipped.")
                                    .small()
                                    .color(ui.visuals().weak_text_color()),
                            );
                            ui.end_row();
                        }

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
                    let mut text = if p > 0.03 && p < 1.0 {
                        format!("{:.0}%  ·  about {} left", p * 100.0, human_secs(elapsed * (1.0 - p) / p))
                    } else {
                        format!("{:.0}%", p * 100.0)
                    };
                    if let Ok(scene) = d.progress_scene.lock()
                        && !scene.is_empty()
                    {
                        text = format!("{scene}  ·  {text}");
                    }
                    ui.add(egui::ProgressBar::new(p).desired_width(ui.available_width()).animate(true).text(text));
                } else if ui
                    .add(
                        egui::Button::new(RichText::new("Compile").strong().size(15.0).color(crate::theme::palette().on_accent))
                            .fill(crate::theme::palette().accent)
                            .min_size(Vec2::new(140.0, 34.0)),
                    )
                    .clicked()
                    && let Some(p) = self.project.clone() {
                        let scope = d.scope;
                        let scene = (scope == Scope::Scene).then(|| d.scene.clone()).flatten();
                        let settings = Settings {
                            format: d.format,
                            resolution: d.resolution,
                            framing: d.framing,
                            fps_override: d.fps,
                            frames: marked.filter(|_| d.only_marked && scope == Scope::Scene && scene.is_some() && scene == active_id),
                            ffmpeg: None,
                        };
                        let (tx, rx) = mpsc::channel();
                        let ctx = ui.ctx().clone();
                        let progress = Arc::new(AtomicU32::new(0));
                        let progress_scene: Arc<Mutex<String>> = Arc::default();
                        let shared = progress.clone();
                        let shared_scene = progress_scene.clone();
                        thread::spawn(move || {
                            let r = if scope == Scope::Each {
                                compile::compile_each(&p, &settings, |f, name| {
                                    shared.store(f.to_bits(), Ordering::Relaxed);
                                    if let Ok(mut s) = shared_scene.lock()
                                        && *s != name
                                    {
                                        *s = name.to_string();
                                    }
                                })
                                .map(CompileDone::Each)
                            } else {
                                compile::compile_with_progress(&p, scene.as_deref(), &settings, |f| {
                                    shared.store(f.to_bits(), Ordering::Relaxed);
                                })
                                .map(CompileDone::One)
                            };
                            let _ = tx.send(r.map_err(|e| e.to_string()));
                            ctx.request_repaint();
                        });
                        d.progress = progress;
                        d.progress_scene = progress_scene;
                        d.started = Instant::now();
                        d.running = Some(rx);
                        d.result = None;
                    }

                match &d.result {
                    Some(Ok(CompileDone::One(out))) => {
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
                    Some(Ok(CompileDone::Each(out))) => {
                        ui.separator();
                        ui.label(format!("Saved {} scenes to", out.files.len()));
                        ui.monospace(out.dir.display().to_string());
                        for f in &out.files {
                            let name = f.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                            ui.label(RichText::new(format!("· {name}  ({} frames)", f.frames)).small());
                        }
                        for w in &out.warnings {
                            ui.label(RichText::new(format!("Warning: {w}")).small());
                        }
                        if ui.button("Show in folder").clicked() {
                            reveal(&out.files[0].path);
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
}
