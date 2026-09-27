//! Tools → Import images…: pull photos from a folder or camera card into a scene.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::thread;

use dragonslayer_core::import::{self, Order, Plan, Report};
use dragonslayer_core::Scene;
use eframe::egui::{self, RichText, Vec2};

use super::DragonSlayerApp;

#[derive(Default)]
pub(super) struct ImportDialog {
    pub(super) open: bool,
    sources: Vec<PathBuf>,
    order: Order,
    /// Add after the active scene's frames instead of making a new scene.
    into_active: bool,
    new_name: String,
    scanning: Option<Receiver<Result<Plan, String>>>,
    plan: Option<Result<Plan, String>>,
    running: Option<Receiver<Report>>,
    done: Arc<AtomicUsize>,
    total: usize,
    stop: Arc<AtomicBool>,
    /// (display name, frames found by the scan) of the import in progress or just finished.
    target: Option<String>,
    report: Option<Report>,
    /// Kept from the plan for the results screen: folders that couldn't be read at all.
    unreadable: Vec<(PathBuf, String)>,
    raw_only: usize,
}

impl ImportDialog {
    pub(super) fn is_running(&self) -> bool {
        self.running.is_some()
    }

    #[cfg(test)]
    pub(super) fn report(&self) -> Option<&Report> {
        self.report.as_ref()
    }

    #[cfg(test)]
    pub(super) fn found(&self) -> Option<usize> {
        match &self.plan {
            Some(Ok(p)) => Some(p.frames.len()),
            _ => None,
        }
    }

    #[cfg(test)]
    pub(super) fn set_into_active(&mut self, yes: bool) {
        self.into_active = yes;
    }
}

impl DragonSlayerApp {
    /// Tools menu: pick the card or folder and open the dialog.
    pub(super) fn start_import(&mut self, ctx: &egui::Context) {
        if self.project.is_none() || self.import.is_running() {
            return;
        }
        let Some(folder) = rfd::FileDialog::new()
            .set_title("Choose the memory card or folder with the photos (e.g. its DCIM folder)")
            .pick_folder()
        else {
            return;
        };
        self.open_import(vec![folder], ctx);
    }

    /// Opens the dialog on `sources` and starts scanning them.
    pub(super) fn open_import(&mut self, sources: Vec<PathBuf>, ctx: &egui::Context) {
        self.interval = None;
        self.import = ImportDialog { open: true, sources, new_name: "Imported".into(), ..Default::default() };
        self.scan_import(ctx);
    }

    pub(super) fn scan_import(&mut self, ctx: &egui::Context) {
        let d = &mut self.import;
        d.plan = None;
        let scene: Option<Scene> = if d.into_active {
            self.project.as_ref().and_then(|p| p.active_scene().ok())
        } else {
            None
        };
        let (sources, order) = (d.sources.clone(), d.order);
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        thread::spawn(move || {
            let _ = tx.send(import::scan(&sources, order, scene.as_ref()).map_err(|e| e.to_string()));
            ctx.request_repaint();
        });
        d.scanning = Some(rx);
    }

    fn run_import(&mut self, ctx: &egui::Context) {
        let Some(Ok(plan)) = self.import.plan.take() else { return };
        let target = if self.import.into_active {
            self.project.as_ref().and_then(|p| p.active_scene().ok())
        } else {
            let name = self.import.new_name.trim().to_owned();
            let name = if name.is_empty() { "Imported".to_owned() } else { name };
            let after = self.active_row().map(|r| r.id.clone());
            let Some(p) = self.project.as_mut() else { return };
            match p.add_scene(&name, after.as_deref()) {
                Ok(s) => Some(s),
                Err(e) => {
                    self.error(e);
                    None
                }
            }
        };
        self.refresh();
        let Some(scene) = target else { return };

        let d = &mut self.import;
        d.target = Some(scene.name().to_owned());
        d.total = plan.frames.len();
        d.raw_only = plan.raw_only();
        d.unreadable = plan.unreadable.clone();
        d.done = Arc::new(AtomicUsize::new(0));
        d.stop = Arc::new(AtomicBool::new(false));
        let (done, stop) = (d.done.clone(), d.stop.clone());
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        thread::spawn(move || {
            let report = import::import(&scene, &plan, |n, _| {
                done.store(n, Ordering::Relaxed);
                ctx.request_repaint();
                !stop.load(Ordering::Relaxed)
            });
            let _ = tx.send(report);
            ctx.request_repaint();
        });
        d.running = Some(rx);
    }

    pub(super) fn import_window(&mut self, ctx: &egui::Context) {
        if !self.import.open {
            return;
        }
        use egui_phosphor::regular as ph;
        let pal = crate::theme::palette();

        if let Some(rx) = &self.import.scanning
            && let Ok(plan) = rx.try_recv()
        {
            self.import.scanning = None;
            self.import.plan = Some(plan);
        }
        if let Some(rx) = &self.import.running
            && let Ok(report) = rx.try_recv()
        {
            self.import.running = None;
            let n = report.imported.len();
            self.import.report = Some(report);
            self.refresh();
            self.info(format!("Imported {n} frames"));
        }

        let mut rescan = false;
        let mut start = false;
        let mut close = false;
        let mut pick_files = false;
        let mut pick_folder = false;
        let modal = egui::Modal::new(egui::Id::new("import modal")).show(ctx, |ui| {
            ui.set_width(560.0);
            ui.heading(format!("{}  Import images", ph::DOWNLOAD_SIMPLE));
            ui.label(
                RichText::new(
                    "Pull photos from a camera card or folder into a scene, for example to rebuild a \
                     film from the card after a project was lost. The card is only read, never changed.",
                )
                .color(pal.text_muted),
            );
            ui.separator();
            let d = &mut self.import;

            // Results.
            if let Some(r) = &d.report {
                let into = d.target.as_deref().unwrap_or("the scene");
                ui.label(RichText::new(format!("Imported {} frames into {into}.", r.imported.len())).strong());
                if r.stopped {
                    ui.label("Stopped before the end. Run the import again to carry on: frames already imported are skipped.");
                }
                if let Some(why) = &r.aborted {
                    ui.colored_label(pal.error, format!("Stopped early: {why}"));
                }
                if !r.failed.is_empty() {
                    ui.add_space(4.0);
                    ui.colored_label(pal.warn, format!("{} shots couldn't be read and were skipped:", r.failed.len()));
                    list(ui, r.failed.iter().map(|(s, why)| format!("{s}: {why}")));
                    ui.label(
                        RichText::new("If the card is damaged, try copying those files off it with the computer first, then import that folder: the rest are skipped.")
                            .small()
                            .color(pal.text_muted),
                    );
                }
                if !r.truncated.is_empty() {
                    ui.add_space(4.0);
                    ui.colored_label(pal.warn, "These frames look cut short (the card may be damaged). Check them in Preview:");
                    list(ui, r.truncated.iter().map(|(f, s)| format!("frame {f}  ({s})")));
                }
                if d.raw_only > 0 {
                    ui.label(format!(
                        "{} shots were RAW only: they're kept in the scene's frames folder, but can't be shown or compiled.",
                        d.raw_only
                    ));
                }
                if !d.unreadable.is_empty() {
                    ui.colored_label(pal.warn, "Some folders couldn't be read:");
                    list(ui, d.unreadable.iter().map(|(p, why)| format!("{}: {why}", p.display())));
                }
                ui.separator();
                if ui.button("Close").clicked() {
                    close = true;
                }
                return;
            }

            // Progress.
            if d.running.is_some() {
                let n = d.done.load(Ordering::Relaxed);
                let p = if d.total == 0 { 0.0 } else { n as f32 / d.total as f32 };
                ui.label(format!("Importing into {}…", d.target.as_deref().unwrap_or("the scene")));
                ui.add(
                    egui::ProgressBar::new(p)
                        .desired_width(ui.available_width())
                        .animate(true)
                        .text(format!("{n} / {}", d.total)),
                );
                ui.add_space(4.0);
                let stopping = d.stop.load(Ordering::Relaxed);
                if ui.add_enabled(!stopping, egui::Button::new(format!("{}  Stop", ph::STOP))).clicked() {
                    d.stop.store(true, Ordering::Relaxed);
                }
                return;
            }

            // Source and options.
            ui.horizontal(|ui| {
                ui.label("From");
                let shown = match d.sources.as_slice() {
                    [one] => one.display().to_string(),
                    many => format!("{} files", many.len()),
                };
                ui.monospace(shown);
            });
            ui.horizontal(|ui| {
                if ui.small_button("Choose folder…").clicked() {
                    pick_folder = true;
                }
                if ui.small_button("Choose files…").clicked() {
                    pick_files = true;
                }
            });
            ui.add_space(6.0);
            egui::Grid::new("import options").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                ui.label("Into");
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        if ui.radio(!d.into_active, "A new scene called").clicked() && d.into_active {
                            d.into_active = false;
                            rescan = true;
                        }
                        ui.add_enabled(!d.into_active, egui::TextEdit::singleline(&mut d.new_name).desired_width(160.0));
                    });
                    if ui.radio(d.into_active, "The active scene, after its frames").clicked() && !d.into_active {
                        d.into_active = true;
                        rescan = true;
                    }
                });
                ui.end_row();
                ui.label("Order");
                ui.horizontal(|ui| {
                    if ui.radio_value(&mut d.order, Order::Taken, "When taken").changed() {
                        rescan = true;
                    }
                    if ui.radio_value(&mut d.order, Order::Name, "File name").changed() {
                        rescan = true;
                    }
                });
                ui.end_row();
            });
            ui.label(
                RichText::new("\"When taken\" follows the camera's clock and copes with its file numbers starting again at 0001.")
                    .small()
                    .color(pal.text_muted),
            );
            ui.separator();

            let mut can_start = false;
            if d.scanning.is_some() {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Looking for photos…");
                });
            } else {
                match &d.plan {
                    None => {}
                    Some(Err(e)) => {
                        ui.colored_label(pal.error, e);
                    }
                    Some(Ok(plan)) => {
                        let found = plan.frames.len();
                        ui.label(RichText::new(format!("{found} shots to import")).strong());
                        if let (Some(first), Some(last)) = (plan.frames.first(), plan.frames.last()) {
                            ui.label(RichText::new(format!("{}  …  {}", first.source, last.source)).monospace().small());
                        }
                        if plan.raw_only() > 0 {
                            ui.label(format!("{} of them are RAW only (no JPEG): kept, but not shown or compiled.", plan.raw_only()));
                        }
                        if plan.already_imported > 0 {
                            ui.label(format!("{} already in this scene: skipped.", plan.already_imported));
                        }
                        if plan.ignored > 0 {
                            ui.label(format!("{} other files (videos, thumbnails) ignored.", plan.ignored));
                        }
                        if !plan.unreadable.is_empty() {
                            ui.colored_label(pal.warn, format!("{} folders or files couldn't be read:", plan.unreadable.len()));
                            list(ui, plan.unreadable.iter().map(|(p, why)| format!("{}: {why}", p.display())));
                        }
                        can_start = found > 0;
                        if found == 0 {
                            ui.label("Nothing new to import from here.");
                        }
                    }
                }
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let import = egui::Button::new(RichText::new("Import").strong().color(pal.on_accent))
                    .fill(pal.accent)
                    .min_size(Vec2::new(120.0, 30.0));
                if ui.add_enabled(can_start, import).clicked() {
                    start = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });

        if modal.should_close() && !self.import.is_running() {
            close = true;
        }
        if pick_folder && let Some(f) = rfd::FileDialog::new().pick_folder() {
            self.import.sources = vec![f];
            rescan = true;
        }
        if pick_files
            && let Some(files) = rfd::FileDialog::new()
                .add_filter("Photos", &["jpg", "jpeg", "cr2", "cr3", "nef", "arw", "rw2", "raf", "orf", "pef", "dng"])
                .pick_files()
        {
            self.import.sources = files;
            rescan = true;
        }
        if rescan {
            self.scan_import(ctx);
        }
        if start {
            self.run_import(ctx);
        }
        if close {
            self.import = ImportDialog::default();
        }
    }
}

/// A short scrollable list, so a badly damaged card can't push the buttons off-screen.
fn list(ui: &mut egui::Ui, items: impl Iterator<Item = String>) {
    egui::ScrollArea::vertical().max_height(110.0).id_salt(ui.next_auto_id()).show(ui, |ui| {
        for item in items {
            ui.label(RichText::new(item).monospace().small());
        }
    });
}
