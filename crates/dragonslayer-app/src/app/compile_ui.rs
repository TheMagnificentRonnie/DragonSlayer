//! The Compile video window.

use super::*;

impl DragonSlayerApp {
    pub(super) fn compile_window(&mut self, ctx: &egui::Context) {
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
}
