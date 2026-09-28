//! The viewer (live view, onion skin, preview), filmstrip timeline and minimal view.

use super::*;

impl DragonSlayerApp {
    /// Why `frame` can't be shown, if it can't: RAW only (nothing to display), or a JPEG
    /// that couldn't be decoded. `None` when it's fine or still loading.
    pub(super) fn frame_problem(&self, frame: &Frame, width: u32) -> Option<String> {
        match frame.jpeg() {
            None => {
                let raw = frame.files.first().and_then(|f| f.file_name()).map(|n| n.to_string_lossy().into_owned());
                Some(format!(
                    "Frame {} is RAW only ({}), so there's nothing to show or compile.\n\
                     DragonSlayer works from the JPEG: set Image format to RAW + JPEG\n\
                     in the Exposure panel (or on the camera) for the next shots.",
                    frame.id,
                    raw.as_deref().unwrap_or("no JPEG"),
                ))
            }
            Some(p) if self.images.failed(p, width) => Some(format!(
                "Couldn't read frame {} ({}). The file may be damaged or still being written.",
                frame.id,
                p.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()
            )),
            Some(_) => None,
        }
    }

    /// Loads the frames around the one in Preview in the background: a second's worth ahead
    /// while playing, a few either side while scrubbing.
    fn prefetch_around(&mut self, index: usize, playing: bool, width: u32) {
        let n = self.frames.len();
        if n == 0 {
            return;
        }
        let fps = self
            .project
            .as_ref()
            .and_then(|p| p.active_scene().ok().map(|s| p.fps_for(&s)))
            .unwrap_or(12) as usize;
        let (ahead, behind) = if playing { (fps.clamp(6, 30), 0) } else { (6, 3) };
        // While playing, stay inside what's being played (the marked range, or everything).
        let (lo, hi) = if playing { self.play_bounds() } else { (0, n - 1) };
        let wanted = (index + 1..=(index + ahead).min(hi)).chain(index.saturating_sub(behind).max(lo)..index);
        for i in wanted {
            if let Some(p) = self.frames[i].jpeg() {
                self.images.prefetch(p, width);
            }
        }
        // Looping wraps to the in point (or the first frame): have those ready too.
        if playing && self.loop_on && index + ahead > hi {
            for f in &self.frames[lo..=(lo + ahead).min(hi)] {
                if let Some(p) = f.jpeg() {
                    self.images.prefetch(p, width);
                }
            }
        }
    }

    pub(super) fn viewer(&mut self, ui: &mut egui::Ui) {
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
                self.prefetch_around(index, playing, width);
                let base = self.frames.get(index).and_then(|f| f.jpeg()).and_then(|p| self.images.get(p, width));
                let cant_show = self.frames.get(index).and_then(|f| self.frame_problem(f, width));
                // While the next frame decodes, keep showing the last one rather than a blank
                // "Loading…" that flickers during scrubbing and playback. A frame that can't be
                // shown at all says why instead of holding a misleading picture.
                let (base, loading) = match base {
                    Some(t) => {
                        self.preview_hold = Some(t.clone());
                        (Some(t), false)
                    }
                    None if cant_show.is_some() => (None, false),
                    None => (self.preview_hold.clone(), true),
                };
                let mut label = if playing {
                    format!("PLAY {}/{}", index + 1, n.max(1))
                } else {
                    format!("PREVIEW {}/{}", index + 1, n.max(1))
                };
                if loading {
                    label.push_str(" · loading");
                }
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
            // The frame the viewer is trying to show (Preview's, or the last one without live view).
            let shown = match self.mode {
                Mode::Preview { index, .. } => self.frames.get(index),
                Mode::Capture => self.frames.last(),
            };
            let text = if self.frames.is_empty() {
                match &self.status {
                    Status::Connected { .. } => "Waiting for live view…".to_string(),
                    _ => "Connect a camera to see live view".to_string(),
                }
            } else if let Some(problem) = shown.and_then(|f| self.frame_problem(f, width)) {
                problem
            } else {
                "Loading…".to_string()
            };
            // A real label (not painted text) so screen readers and the UI tests can read it.
            let label = egui::Label::new(RichText::new(text).size(17.0).color(Color32::from_gray(185)));
            ui.put(area.shrink(24.0), label);
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
    pub(super) fn timeline(&mut self, ui: &mut egui::Ui) {
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

        let marked = self.marked_range();
        for (draw_i, i) in (start..end).enumerate() {
            let x = rect.left() + 4.0 + draw_i as f32 * stride;
            let r = Rect::from_min_size(egui::pos2(x, rect.top() + 5.0), Vec2::new(thumb_w, thumb_h));
            let is_current = i == cur && !self.mode.is_capture();
            painter.rect_filled(r, 2.0, Color32::from_gray(35));
            if let Some(tex) = self.frames[i].jpeg().and_then(|p| self.images.get(p, 160)) {
                let inner = fit(r.shrink(2.0), tex.size_vec2());
                painter.image(tex.id(), inner, Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
            } else if self.frames[i].jpeg().is_none() {
                painter.text(r.center(), Align2::CENTER_CENTER, "RAW\nonly", FontId::proportional(11.0), crate::theme::palette().warn);
            }
            // The marked range: a tinted band across its frames and a bar at each end.
            if let Some((a, b)) = marked
                && (a..=b).contains(&i)
            {
                let accent = crate::theme::palette().accent;
                let band = Rect::from_min_max(egui::pos2(r.left() - gap / 2.0, r.top() - 3.0), egui::pos2(r.right() + gap / 2.0, r.top()));
                painter.rect_filled(band, 0.0, accent);
                painter.rect_filled(r, 2.0, accent.gamma_multiply(0.18));
                let bar = |x: f32| Rect::from_min_max(egui::pos2(x - 1.5, r.top() - 3.0), egui::pos2(x + 1.5, r.bottom()));
                if i == a {
                    painter.rect_filled(bar(r.left() - gap / 2.0), 0.0, accent);
                }
                if i == b {
                    painter.rect_filled(bar(r.right() + gap / 2.0), 0.0, accent);
                }
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

    /// Floating control bar for minimal view. Fades out when the mouse is still,
    /// stays while hovered.
    pub(super) fn minimal_bar(&mut self, ctx: &egui::Context) {
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
    pub(super) fn toggle_minimal(&mut self) {
        if self.project.is_none() {
            return;
        }
        self.minimal = !self.minimal;
        if self.minimal {
            self.mode = Mode::Capture;
        }
    }

    // ---- navigator ------------------------------------------------------
}
