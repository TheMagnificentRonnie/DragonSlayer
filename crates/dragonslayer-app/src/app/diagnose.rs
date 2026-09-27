//! Camera status, the background USB check and the Diagnose camera window.

use super::*;

impl DragonSlayerApp {
    /// Checks USB devices in the background (a PowerShell query, ~2 s). Windows only.
    pub(super) fn start_usb_scan(&mut self, ctx: &egui::Context) {
        if !cfg!(windows) || self.usb_scan_rx.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        thread::spawn(move || {
            let _ = tx.send(dragonslayer_camera::diag::scan());
            ctx.request_repaint();
        });
        self.usb_scan_rx = Some(rx);
        self.usb_scan_at = Some(Instant::now());
    }

    /// While no camera is connected, re-check USB every 10 s so a camera stuck on
    /// Windows' own driver (wrong port for Zadig) is pointed out without asking.
    pub(super) fn auto_usb_scan(&mut self, ctx: &egui::Context) {
        let looking = matches!(self.status, Status::Searching | Status::Problem { .. } | Status::WrongDriver { .. });
        if looking && self.usb_scan_at.is_none_or(|t| t.elapsed() >= Duration::from_secs(10)) {
            self.start_usb_scan(ctx);
        }
    }

    /// A plugged-in camera on Windows' own driver, according to the last USB check.
    pub(super) fn camera_on_windows_driver(&self) -> Option<&UsbCamera> {
        self.usb_scan.as_ref()?.as_ref().ok()?.iter().find(|c| !c.driver.usable())
    }

    pub(super) fn camera_status(&mut self, ui: &mut egui::Ui) {
        // Actionable driver state gets its own row with a Set up button. The USB check
        // catches it even when the camera library doesn't list the camera at all.
        let needs_driver = match &self.status {
            Status::WrongDriver { name } => Some(name.clone().unwrap_or_else(|| "Camera".into())),
            Status::Searching | Status::Problem { .. } => self.camera_on_windows_driver().map(|c| c.name.clone()),
            _ => None,
        };
        if let Some(n) = needs_driver {
            if ui
                .small_button(egui_phosphor::regular::STETHOSCOPE)
                .on_hover_text("Diagnose camera")
                .clicked()
            {
                self.diagnose_open = true;
                self.start_usb_scan(ui.ctx());
            }
            if ui
                .button("Set up USB driver…")
                .on_hover_text("One-time driver swap so DragonSlayer can talk to the camera")
                .clicked()
            {
                launch_driver_setup();
            }
            ui.label(RichText::new(format!("{n} · this USB port needs driver setup")).color(crate::theme::palette().warn));
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
            ui.painter().circle_filled(rect.center(), 5.0, crate::theme::palette().warn);
            return;
        }
        let (dot, text, detail) = match &self.status {
            Status::Connected { name, caps } => (
                crate::theme::palette().ok,
                format!("{name} · connected"),
                (!caps.live_view).then(|| "No live view on this model: onion skin shows over the last frame.".to_string()),
            ),
            Status::Searching => (
                crate::theme::palette().warn,
                "No camera".into(),
                Some(connect_hint().to_string()),
            ),
            Status::NoBackend => (
                Color32::GRAY,
                "No camera support in this build".into(),
                Some("Rebuild with `--features gphoto2`, or start with --mock to try the mock camera.".into()),
            ),
            Status::Problem { name, message } => (
                crate::theme::palette().error,
                name.clone().unwrap_or_else(|| "Camera".into()) + " · problem",
                Some(message.clone()),
            ),
            Status::WrongDriver { .. } => unreachable!(),
        };
        if matches!(self.status, Status::Searching | Status::Problem { .. })
            && ui
                .small_button(egui_phosphor::regular::STETHOSCOPE)
                .on_hover_text("Diagnose camera")
                .clicked()
        {
            self.diagnose_open = true;
            self.start_usb_scan(ui.ctx());
        }
        if let Some(detail) = &detail {
            ui.label(RichText::new(detail).small().color(ui.visuals().weak_text_color())).on_hover_text(detail);
        }
        ui.label(text);
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
        ui.painter().circle_filled(rect.center(), 5.0, dot);
    }

    /// Camera diagnosis: each check with a verdict and, when it fails, the fix.
    pub(super) fn diagnose_window(&mut self, ctx: &egui::Context) {
        if !self.diagnose_open {
            return;
        }
        use egui_phosphor::regular as ph;
        #[derive(Clone, Copy)]
        enum V {
            Ok,
            Warn,
            Fail,
            Info,
        }
        let pal = crate::theme::palette();
        let row = |ui: &mut egui::Ui, v: V, title: &str, fix: &str| {
            let (icon, color) = match v {
                V::Ok => (ph::CHECK_CIRCLE, pal.ok),
                V::Warn => (ph::WARNING, pal.warn),
                V::Fail => (ph::X_CIRCLE, pal.error),
                V::Info => (ph::INFO, pal.text_muted),
            };
            ui.horizontal_top(|ui| {
                ui.label(RichText::new(icon).size(16.0).color(color));
                ui.vertical(|ui| {
                    ui.label(RichText::new(title).strong());
                    if !fix.is_empty() {
                        ui.label(RichText::new(fix).color(pal.text_muted));
                    }
                });
            });
            ui.add_space(4.0);
        };

        let mut open = true;
        let mut rescan = false;
        let mut advanced = false;
        let mut any_wrong_driver = matches!(self.status, Status::WrongDriver { .. });
        egui::Window::new(format!("{}  Camera diagnosis", ph::STETHOSCOPE))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(520.0)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.set_max_width(520.0);
                ui.label(RichText::new("CAMERA LIBRARY").small().strong().color(pal.text_muted));
                let reset = "Do the full reset: quit DragonSlayer, turn the camera off (battery out if it \
                    won't respond), turn it on, then reopen DragonSlayer.";
                match &self.status {
                    Status::Connected { name, .. } => row(ui, V::Ok, &format!("DragonSlayer is talking to the {name}"), ""),
                    Status::Searching => row(ui, V::Fail, "No camera found", connect_hint()),
                    Status::WrongDriver { name } => row(
                        ui,
                        V::Fail,
                        &format!("{} is on Windows' own driver", name.as_deref().unwrap_or("The camera")),
                        "This USB port hasn't been set up for DragonSlayer. Click Set up USB driver… and \
                        replace the driver with WinUSB, or move the camera back to the port you set up before.",
                    ),
                    Status::Problem { name, message } => row(
                        ui,
                        V::Fail,
                        &format!("{}: {message}", name.as_deref().unwrap_or("Camera")),
                        reset,
                    ),
                    Status::NoBackend => row(
                        ui,
                        V::Fail,
                        "This build has no camera support",
                        "Download the release build from GitHub, or rebuild with --features gphoto2.",
                    ),
                }

                if let Status::Connected { caps, .. } = &self.status {
                    if self.camera_settings.is_empty() {
                        row(ui, V::Info, "The camera didn't report any settings", "Some models don't; capture still works.");
                    } else {
                        row(
                            ui,
                            V::Ok,
                            &format!("The camera answers commands ({} settings read)", self.camera_settings.len()),
                            "",
                        );
                    }
                    let live_recent = self.last_live_at.is_some_and(|t| t.elapsed() < Duration::from_secs(3));
                    if !caps.live_view {
                        row(ui, V::Info, "This model has no live view over USB", "Onion skin shows over the last frame instead.");
                    } else if !self.mode.is_capture() {
                        row(ui, V::Info, "Live view is paused while you're in Preview", "Press Tab to go back to Capture.");
                    } else if live_recent {
                        row(ui, V::Ok, "Live view frames are arriving", "");
                    } else {
                        row(
                            ui,
                            V::Fail,
                            "No live view frames",
                            "Canon: mode dial on M and Live View shooting enabled in the menu. Check the battery. \
                            If it still fails, do the full reset (quit DragonSlayer first).",
                        );
                    }
                }

                ui.add_space(6.0);
                ui.label(RichText::new("USB").small().strong().color(pal.text_muted));
                if !cfg!(windows) {
                    row(
                        ui,
                        V::Info,
                        "USB driver checks are for Windows",
                        "On macOS: quit Photos and Image Capture. If the camera was plugged in after \
                        DragonSlayer opened, quit and reopen DragonSlayer.",
                    );
                } else if self.usb_scan_rx.is_some() {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Checking USB devices…");
                    });
                } else {
                    match &self.usb_scan {
                        None => {}
                        Some(Err(e)) => row(ui, V::Info, "Couldn't check the USB devices", e),
                        Some(Ok(cams)) if cams.is_empty() => row(
                            ui,
                            V::Fail,
                            "No camera seen on USB at all",
                            "Is the camera on? Some USB cables only charge: try another cable. Check the \
                            camera's USB mode is PC / PTP, not mass storage.",
                        ),
                        Some(Ok(cams)) => {
                            for c in cams {
                                match &c.driver {
                                    dragonslayer_camera::diag::Driver::Usable(d) => {
                                        row(ui, V::Ok, &format!("{}: driver {d}", c.name), "")
                                    }
                                    dragonslayer_camera::diag::Driver::WindowsOwn(d) => {
                                        any_wrong_driver = true;
                                        row(
                                            ui,
                                            V::Fail,
                                            &format!("{} is on Windows' own driver ({d})", c.name),
                                            "Zadig's setup belongs to one USB port, and this one hasn't been set \
                                            up. Click Set up USB driver… (Options → List All Devices, pick the \
                                            camera, WinUSB, Replace Driver), or move the camera back to the port \
                                            you set up before.",
                                        );
                                    }
                                }
                                match &c.hub {
                                    Some(hub) => row(
                                        ui,
                                        V::Warn,
                                        &format!("{} is plugged into a hub ({hub})", c.name),
                                        "Hubs are the most common cause of timeouts. If the camera drops or \
                                        times out, plug it straight into the computer (and run Zadig once for \
                                        that port).",
                                    ),
                                    None => row(ui, V::Ok, &format!("{} is plugged straight into the computer", c.name), ""),
                                }
                            }
                        }
                    }
                }

                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button(format!("{}  Check again", ph::ARROW_CLOCKWISE)).clicked() {
                        rescan = true;
                    }
                    if cfg!(windows) && any_wrong_driver && ui.button("Set up USB driver…").clicked() {
                        launch_driver_setup();
                    }
                    if ui.button("Advanced troubleshooting").clicked() {
                        advanced = true;
                    }
                });
            });
        if rescan {
            self.start_usb_scan(ctx);
        }
        if advanced {
            self.help_open = true;
            self.help_tab = HelpTab::Advanced;
            self.diagnose_open = false;
        }
        if !open {
            self.diagnose_open = false;
        }
    }
}
