//! Interval capture: N frames, S seconds apart.

use super::*;

impl DragonSlayerApp {
    /// Kicks off an N-shot interval sequence. First shot fires immediately;
    /// subsequent shots fire `every` seconds after the previous one COMPLETES,
    /// so a slow camera slides the schedule instead of pileup.
    pub(super) fn start_interval(&mut self) {
        if self.interval.is_some() || self.interval_count == 0 || !self.mode.is_capture() {
            return;
        }
        self.interval =
            Some(Interval { total: self.interval_count, done: 0, every: self.interval_secs, next_at: Some(Instant::now()) });
        self.tick_interval();
    }

    pub(super) fn stop_interval(&mut self, reason: &str) {
        if let Some(iv) = self.interval.take() {
            self.info(format!("Interval stopped ({reason}): {}/{} frames captured", iv.done, iv.total));
        }
    }

    /// If an interval is running and the next scheduled time has arrived, fire
    /// the next capture. Called from `update()` every repaint and after each
    /// `Event::Captured`.
    pub(super) fn tick_interval(&mut self) {
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

    /// Interval-capture controls: N frames, every S seconds. Start/Stop toggles.
    pub(super) fn interval_ui(&mut self, ui: &mut egui::Ui) {
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
}
