//! Drives the real app headlessly (egui_kittest) with the mock camera: keys, buttons,
//! dialogs, camera loss. Keys go through `take_shortcuts` exactly as the real window's
//! `raw_input_hook` sends them.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use dragonslayer_camera::mock::MockBackend;
use dragonslayer_core::Project;
use eframe::egui::{self, Key, Modifiers};
use egui_kittest::kittest::Queryable;
use egui_kittest::Harness;

use super::*;

/// CI machines are slow and every test runs its own camera threads in parallel. The Mac
/// runner is roughly half the speed of a laptop, so this needs plenty of headroom.
const WAIT: Duration = Duration::from_secs(90);

struct Rig<'a> {
    h: Harness<'a, DragonSlayerApp>,
    unplugged: Arc<AtomicBool>,
    root: PathBuf,
    _tmp: tempfile::TempDir,
}

fn rig<'a>() -> Rig<'a> {
    rig_with(true, |_| {})
}

/// A harness running the app on a fresh project with the mock camera.
fn rig_with<'a>(open_project: bool, prepare: impl FnOnce(&Path)) -> Rig<'a> {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("Film");
    Project::create(&root, "Film", 12).unwrap();
    prepare(&root);
    let backend = MockBackend::default();
    let unplugged = backend.unplugged.clone();
    let project = open_project.then(|| root.clone());
    let h = Harness::builder()
        .with_size(egui::vec2(1400.0, 900.0))
        .build_eframe(move |cc| DragonSlayerApp::new(cc, Some(Box::new(backend)), project));
    Rig { h, unplugged, root, _tmp: tmp }
}

impl Rig<'_> {
    fn app(&self) -> &DragonSlayerApp {
        self.h.state()
    }

    fn app_mut(&mut self) -> &mut DragonSlayerApp {
        self.h.state_mut()
    }

    /// Steps frames (sleeping a little: the camera runs on real threads and the app uses
    /// the real clock) until `cond` holds.
    #[track_caller]
    fn wait_for(&mut self, what: &str, cond: impl Fn(&DragonSlayerApp) -> bool) {
        let deadline = Instant::now() + WAIT;
        while Instant::now() < deadline {
            self.h.step();
            if cond(self.h.state()) {
                return;
            }
            std::thread::sleep(Duration::from_millis(15));
        }
        panic!("timed out waiting for: {what}");
    }

    fn connected(mut self) -> Self {
        self.wait_for("mock camera to connect", |a| a.camera_ready());
        self
    }

    fn settle(&mut self, frames: usize) {
        for _ in 0..frames {
            self.h.step();
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn press(&mut self, key: Key) {
        self.press_mods(key, Modifiers::NONE);
    }

    /// What the real window does: shortcuts are taken out of the raw input first, the rest
    /// goes to egui.
    fn press_mods(&mut self, key: Key, modifiers: Modifiers) {
        let typing = self.h.ctx.text_edit_focused();
        let mut raw = egui::RawInput::default();
        for pressed in [true, false] {
            raw.events.push(egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers });
        }
        self.app_mut().take_shortcuts(&mut raw, typing);
        for e in raw.events {
            self.h.event(e);
        }
        self.h.step();
    }

    /// Clicks the button labelled `label` (buttons often carry an icon before the text,
    /// so "⊕  Add scene" matches "Add scene"). Several matches: the first, since in this
    /// app duplicates are the same action shown in two places.
    #[track_caller]
    fn click(&mut self, label: &str) {
        use egui_kittest::kittest::By;
        let exact = self.h.query_all(By::new().role(egui::accesskit::Role::Button).label(label)).next();
        let node = exact.unwrap_or_else(|| {
            self.h
                .query_all(By::new().role(egui::accesskit::Role::Button).label_contains(label))
                .next()
                .unwrap_or_else(|| panic!("no button labelled {label:?}"))
        });
        node.click();
        self.h.step();
        self.h.step();
    }

    fn has_label(&self, text: &str) -> bool {
        self.h.query_by_label_contains(text).is_some()
    }

    fn frames(&self) -> usize {
        self.app().frames.len()
    }

    fn capture_frames(&mut self, n: usize) {
        for i in 0..n {
            self.press(Key::Space);
            let want = self.frames() + 1;
            let _ = i;
            self.wait_for("capture to land", move |a| a.frames.len() >= want && !a.capturing);
        }
    }

    fn message(&self) -> String {
        self.app().message.as_ref().map(|m| m.0.clone()).unwrap_or_default()
    }

    fn active_scene_dir(&self) -> PathBuf {
        self.app().project.as_ref().unwrap().active_scene().unwrap().dir
    }
}

// ------------------------------------------------------------------ camera

#[test]
fn mock_camera_connects_and_live_view_frames_arrive() {
    let mut r = rig().connected();
    r.wait_for("live view texture", |a| a.live.is_some() && a.last_live_at.is_some());
    assert!(r.app().has_live_view());
}

#[test]
fn camera_settings_are_read_on_connect() {
    let mut r = rig().connected();
    r.wait_for("settings", |a| a.camera_settings.len() == 6);
}

#[test]
fn changing_a_setting_goes_to_the_camera_and_comes_back() {
    let mut r = rig().connected();
    r.wait_for("settings", |a| !a.camera_settings.is_empty());
    let _ = r.app().session.cmd.send(Cmd::SetSetting(dragonslayer_camera::SettingKind::Iso, "800".into()));
    r.app_mut().setting_pending = true;
    r.wait_for("ISO 800", |a| {
        !a.setting_pending
            && a.camera_settings.iter().any(|s| s.kind == dragonslayer_camera::SettingKind::Iso && s.value == "800")
    });
}

#[test]
fn a_refused_setting_shows_an_error_and_keeps_the_old_value() {
    let mut r = rig().connected();
    r.wait_for("settings", |a| !a.camera_settings.is_empty());
    let _ = r.app().session.cmd.send(Cmd::SetSetting(dragonslayer_camera::SettingKind::WhiteBalance, "Tungsten".into()));
    r.wait_for("error message", |a| a.message.as_ref().is_some_and(|m| m.1));
    assert!(r.message().contains("White balance"), "{}", r.message());
    assert!(r
        .app()
        .camera_settings
        .iter()
        .any(|s| s.kind == dragonslayer_camera::SettingKind::WhiteBalance && s.value == "Daylight"));
}

// ----------------------------------------------------------------- capture

#[test]
fn space_captures_a_frame_into_the_active_scene() {
    let mut r = rig().connected();
    r.capture_frames(1);
    let dir = r.active_scene_dir();
    assert!(dir.join("frames/000001.jpg").is_file());
    assert!(dir.join("frames/000001.RAW").is_file());
    assert!(r.message().contains("Frame 000001 saved"), "{}", r.message());
}

#[test]
fn backspace_moves_the_last_frame_to_trash() {
    let mut r = rig().connected();
    r.capture_frames(2);
    r.press(Key::Backspace);
    r.settle(2);
    assert_eq!(r.frames(), 1);
    assert!(r.active_scene_dir().join("trash/000002.jpg").is_file());
}

#[test]
fn backspace_on_an_empty_scene_is_an_error_not_a_crash() {
    let mut r = rig().connected();
    r.press(Key::Backspace);
    r.settle(2);
    assert_eq!(r.frames(), 0);
    assert!(r.app().message.as_ref().is_some_and(|m| m.1), "expected an error message");
}

#[test]
fn space_does_nothing_in_preview() {
    let mut r = rig().connected();
    r.capture_frames(1);
    r.press(Key::Tab);
    assert!(!r.app().mode.is_capture());
    r.press(Key::Space);
    r.settle(20);
    assert_eq!(r.frames(), 1);
    assert!(!r.app().capturing);
}

#[test]
fn no_capture_without_a_camera() {
    let mut r = rig();
    // Straight away: the session thread may not have connected yet; make sure it hasn't.
    r.unplugged.store(true, Ordering::Relaxed);
    r.settle(10);
    r.press(Key::Space);
    r.settle(10);
    assert_eq!(r.frames(), 0);
    assert!(!r.app().capturing);
}

// ------------------------------------------ keys must not leak out of fields

#[test]
fn typing_in_a_field_never_triggers_shortcuts() {
    // The interval Frames box used to eat Backspace as delete-last-frame.
    let mut r = rig().connected();
    r.capture_frames(2);
    let mut raw = egui::RawInput::default();
    for key in [Key::Backspace, Key::Space, Key::O, Key::F] {
        raw.events.push(egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    }
    raw.events.push(egui::Event::Text(" ".into()));
    let before = raw.events.len();
    r.app_mut().take_shortcuts(&mut raw, true);
    assert_eq!(raw.events.len(), before, "every key goes to the field");
    assert!(r.app().keys.is_empty());
    r.settle(3);
    assert_eq!(r.frames(), 2);
}

#[test]
fn shortcuts_are_off_while_a_dialog_or_rename_is_open() {
    let mut r = rig().connected();
    r.capture_frames(1);
    for open in [
        |a: &mut DragonSlayerApp| a.help_open = true,
        |a: &mut DragonSlayerApp| a.diagnose_open = true,
        |a: &mut DragonSlayerApp| a.compile.open = true,
        |a: &mut DragonSlayerApp| a.renaming = Some(("sc010".into(), "x".into())),
    ] {
        open(r.app_mut());
        assert!(!r.app().shortcuts_active());
        let mut raw = egui::RawInput::default();
        raw.events.push(egui::Event::Key { key: Key::Backspace, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
        r.app_mut().take_shortcuts(&mut raw, false);
        assert_eq!(raw.events.len(), 1);
        let a = r.app_mut();
        a.help_open = false;
        a.diagnose_open = false;
        a.compile.open = false;
        a.renaming = None;
    }
    assert_eq!(r.frames(), 1);
}

#[test]
fn ctrl_modified_keys_are_left_alone() {
    let mut r = rig();
    let mut raw = egui::RawInput::default();
    raw.events.push(egui::Event::Key { key: Key::Space, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::CTRL });
    r.app_mut().take_shortcuts(&mut raw, false);
    assert_eq!(raw.events.len(), 1);
    assert!(r.app().keys.is_empty());
}

#[test]
fn held_space_does_not_machine_gun_captures() {
    let mut r = rig();
    let mut raw = egui::RawInput::default();
    raw.events.push(egui::Event::Key { key: Key::Space, physical_key: None, pressed: true, repeat: true, modifiers: Modifiers::NONE });
    r.app_mut().take_shortcuts(&mut raw, false);
    assert!(r.app().keys.is_empty(), "auto-repeat of Space must not capture again");
}

// ------------------------------------------------------- preview & playback

#[test]
fn tab_arrows_home_end_navigate_preview() {
    let mut r = rig().connected();
    r.capture_frames(3);
    r.press(Key::Tab);
    assert_eq!(r.app().mode.preview_index(), Some(2), "Tab opens on the last frame");
    r.press(Key::ArrowLeft);
    assert_eq!(r.app().mode.preview_index(), Some(1));
    r.press(Key::Home);
    assert_eq!(r.app().mode.preview_index(), Some(0));
    r.press(Key::ArrowLeft);
    assert_eq!(r.app().mode.preview_index(), Some(0), "clamped at the first frame");
    r.press(Key::End);
    assert_eq!(r.app().mode.preview_index(), Some(2));
    r.press(Key::ArrowRight);
    assert_eq!(r.app().mode.preview_index(), Some(2), "clamped at the last frame");
    r.press(Key::Escape);
    assert!(r.app().mode.is_capture());
}

#[test]
fn shift_arrow_jumps_ten_frames() {
    let mut r = rig().connected();
    r.capture_frames(12);
    r.press(Key::Home);
    r.press_mods(Key::ArrowRight, Modifiers::SHIFT);
    assert_eq!(r.app().mode.preview_index(), Some(10));
}

#[test]
fn tab_with_no_frames_stays_in_capture() {
    let mut r = rig();
    r.press(Key::Tab);
    assert!(r.app().mode.is_capture());
}

#[test]
fn p_plays_to_the_end_and_pauses_on_the_last_frame() {
    let mut r = rig().connected();
    r.capture_frames(3);
    r.press(Key::P);
    assert!(r.app().mode.is_playing());
    r.wait_for("playback to finish", |a| a.mode == Mode::Preview { index: 2, playing: false, last_advance: match a.mode { Mode::Preview { last_advance, .. } => last_advance, _ => Instant::now() } });
    r.press(Key::P);
    assert!(r.app().mode.is_playing(), "P at the end restarts from the first frame");
    assert_eq!(r.app().mode.preview_index(), Some(0));
}

#[test]
fn preview_does_not_drift_when_not_playing() {
    // Earlier bug: any Preview state auto-advanced after a key press.
    let mut r = rig().connected();
    r.capture_frames(3);
    r.press(Key::Home);
    r.settle(40);
    assert_eq!(r.app().mode.preview_index(), Some(0));
}

#[test]
fn deleting_the_frame_being_previewed_keeps_the_cursor_in_range() {
    let mut r = rig().connected();
    r.capture_frames(2);
    r.press(Key::End);
    r.press(Key::Backspace);
    r.settle(2);
    assert_eq!(r.app().mode.preview_index(), Some(0));
    r.press(Key::Backspace);
    r.settle(2);
    assert!(r.app().mode.is_capture(), "no frames left: back to Capture");
}

#[test]
fn fast_scrubbing_never_gets_stuck_loading_and_never_goes_blank() {
    // Preview used to stick on "Loading…": every frame scrolled past queued a full decode
    // ahead of the one on screen.
    let mut r = rig().connected();
    r.capture_frames(25);
    r.press(Key::Home);
    r.wait_for("first frame shown", |a| a.preview_hold.is_some());
    for _ in 0..24 {
        r.press(Key::ArrowRight);
        assert!(r.app().preview_hold.is_some(), "the viewer should hold the last frame, not go blank");
    }
    assert_eq!(r.app().mode.preview_index(), Some(24));
    let width = r.app().onion_width();
    let last = r.app().frames[24].jpeg().unwrap().to_path_buf();
    r.wait_for("the frame scrubbed to is decoded", move |a| {
        a.preview_hold.as_ref().is_some_and(|t| t.name().contains(&*last.to_string_lossy()) && t.name().contains(&format!("@{width}:")))
    });
}

#[test]
fn playback_shows_every_frame_without_blanking() {
    let mut r = rig().connected();
    r.capture_frames(20);
    r.press(Key::P);
    // Wait for the first frame to be on screen before checking that later frames stay on.
    r.wait_for("first frame decoded", |a| a.preview_hold.is_some());
    let mut seen = std::collections::BTreeSet::new();
    let deadline = Instant::now() + WAIT;
    while r.app().mode.is_playing() && Instant::now() < deadline {
        r.h.step();
        if let Some(i) = r.app().mode.preview_index() {
            seen.insert(i);
        }
        assert!(r.app().preview_hold.is_some(), "viewer blanked during playback");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!r.app().mode.is_playing(), "playback should reach the end");
    assert!(seen.len() >= 15, "playback skipped most frames: saw {seen:?}");
}

#[test]
fn live_view_pauses_in_preview_and_resumes_in_capture() {
    let mut r = rig().connected();
    r.capture_frames(1);
    r.press(Key::Tab);
    r.settle(3);
    assert_eq!(r.app().want_live_cached, Some(false));
    r.press(Key::Tab);
    r.settle(3);
    assert_eq!(r.app().want_live_cached, Some(true));
    let since = Instant::now();
    r.wait_for("live frames again", move |a| a.last_live_at.is_some_and(|t| t > since));
}

// --------------------------------------------------------------- toggles

#[test]
fn o_toggles_onion_skin_and_f_toggles_minimal_view() {
    let mut r = rig();
    let onion = r.app().onion_on;
    r.press(Key::O);
    assert_eq!(r.app().onion_on, !onion);
    r.press(Key::F);
    assert!(r.app().minimal);
    r.press(Key::Tab);
    assert!(r.app().mode.is_capture(), "no Preview from minimal view");
    r.press(Key::Escape);
    assert!(!r.app().minimal);
}

#[test]
fn h_opens_help_and_the_advanced_tab_works() {
    let mut r = rig();
    r.press(Key::H);
    assert!(r.app().help_open);
    r.settle(2);
    r.click("Advanced camera troubleshooting");
    assert!(r.app().help_tab == HelpTab::Advanced);
    assert!(r.has_label("1. Read the message"));
    r.click("Guide");
    assert!(r.has_label("Keyboard shortcuts"));
}

// ---------------------------------------------------------------- interval

#[test]
fn interval_capture_takes_n_frames_then_stops() {
    let mut r = rig().connected();
    {
        let a = r.app_mut();
        a.interval_count = 3;
        a.interval_secs = 0.05;
    }
    r.click("Start interval");
    r.wait_for("3 interval frames", |a| a.frames.len() == 3 && a.interval.is_none());
    assert!(r.message().contains("Interval done"), "{}", r.message());
    r.settle(20);
    assert_eq!(r.frames(), 3, "no extra frames after the interval ends");
}

#[test]
fn switching_to_preview_stops_an_interval() {
    let mut r = rig().connected();
    r.capture_frames(1);
    {
        let a = r.app_mut();
        a.interval_count = 50;
        a.interval_secs = 0.3;
    }
    r.app_mut().start_interval();
    r.wait_for("first interval shot", |a| a.interval.is_some_and(|iv| iv.done >= 1));
    r.press(Key::Tab);
    r.wait_for("interval stopped", |a| a.interval.is_none());
    assert!(r.message().contains("switched to Preview"), "{}", r.message());
}

#[test]
fn unplugging_the_camera_stops_an_interval_and_reconnects_later() {
    let mut r = rig().connected();
    {
        let a = r.app_mut();
        a.interval_count = 100;
        a.interval_secs = 0.2;
    }
    r.app_mut().start_interval();
    r.wait_for("an interval shot", |a| !a.frames.is_empty());
    r.unplugged.store(true, Ordering::Relaxed);
    r.wait_for("interval stopped on camera loss", |a| a.interval.is_none() && !a.camera_ready());
    let frames = r.frames();
    r.settle(20);
    assert_eq!(r.frames(), frames);

    r.unplugged.store(false, Ordering::Relaxed);
    r.wait_for("camera back", |a| a.camera_ready());
    r.capture_frames(1);
    assert_eq!(r.frames(), frames + 1);
}

// ------------------------------------------------------------------ scenes

#[test]
fn add_scene_button_adds_and_activates_a_scene() {
    let mut r = rig();
    r.click("Add scene");
    assert_eq!(r.app().scenes.len(), 2);
    assert_eq!(r.app().active_row().unwrap().name, "Scene 2");
}

#[test]
fn captures_go_to_whichever_scene_is_active() {
    let mut r = rig().connected();
    r.capture_frames(1);
    r.click("Add scene");
    r.capture_frames(2);
    let rows: Vec<usize> = r.app().scenes.iter().map(|s| s.count).collect();
    assert_eq!(rows, [1, 2]);
}

// ------------------------------------------------------------------ import

fn card(dir: &Path) -> PathBuf {
    let dcim = dir.join("DCIM/100CANON");
    fs::create_dir_all(&dcim).unwrap();
    for i in 1..=4 {
        fs::write(dcim.join(format!("IMG_000{i}.JPG")), [0xFF, 0xD8, 1, 2, 0xFF, 0xD9]).unwrap();
    }
    fs::write(dcim.join("IMG_0001.CR2"), b"raw").unwrap();
    dir.join("DCIM")
}

#[test]
#[ignore = "race with egui_kittest modal draw timing; run with --ignored locally"]
fn import_dialog_imports_a_card_into_a_new_scene() {
    let mut r = rig();
    let src = card(r._tmp.path());
    let ctx = r.h.ctx.clone();
    r.app_mut().open_import(vec![src], &ctx);
    r.wait_for("scan", |a| a.import.found() == Some(4));
    assert!(r.has_label("4 shots to import"));
    r.click("Import");
    r.wait_for("import done", |a| a.import.report().is_some());
    assert_eq!(r.app().import.report().unwrap().imported.len(), 4);
    assert_eq!(r.app().scenes.len(), 2);
    assert_eq!(r.app().active_row().unwrap().name, "Imported");
    assert_eq!(r.app().active_row().unwrap().count, 4);
    r.click("Close");
    assert!(!r.app().import.open);
}

#[test]
#[ignore = "race with egui_kittest modal draw timing; run with --ignored locally"]
fn import_into_the_active_scene_skips_what_is_already_there() {
    let mut r = rig();
    let src = card(r._tmp.path());
    let ctx = r.h.ctx.clone();
    let open_into_active = |r: &mut Rig<'_>| {
        r.app_mut().open_import(vec![src.clone()], &ctx);
        r.app_mut().import.set_into_active(true);
        // Same as picking "The active scene" in the dialog: rescan against it.
        r.app_mut().scan_import(&ctx);
    };
    open_into_active(&mut r);
    r.wait_for("scan", |a| a.import.found() == Some(4));
    r.click("Import");
    r.wait_for("import done", |a| a.import.report().is_some());
    assert_eq!(r.app().scenes.len(), 1, "no new scene");
    assert_eq!(r.app().active_row().unwrap().count, 4);
    r.click("Close");

    open_into_active(&mut r);
    r.wait_for("rescan", |a| a.import.found() == Some(0));
    assert!(r.has_label("already in this scene"));
}

#[test]
#[ignore = "race with egui_kittest modal draw timing; run with --ignored locally"]
fn capture_is_blocked_while_an_import_runs() {
    let mut r = rig().connected();
    let src = card(r._tmp.path());
    let ctx = r.h.ctx.clone();
    r.app_mut().open_import(vec![src], &ctx);
    r.wait_for("scan", |a| a.import.found().is_some());
    r.click("Import");
    // Whether or not it has finished yet, a direct capture must not interleave with it.
    if r.app().import.is_running() {
        r.app_mut().capture();
        assert!(!r.app().capturing);
    }
    r.wait_for("import done", |a| a.import.report().is_some());
}

// --------------------------------------------------------------- diagnosis

#[test]
fn diagnosis_reports_a_healthy_mock_camera() {
    let mut r = rig().connected();
    r.wait_for("settings", |a| !a.camera_settings.is_empty());
    r.wait_for("live", |a| a.last_live_at.is_some());
    r.app_mut().diagnose_open = true;
    r.settle(3);
    assert!(r.has_label("DragonSlayer is talking to the DragonSlayer Mock Camera"));
    assert!(r.has_label("The camera answers commands (6 settings read)"));
    assert!(r.has_label("Live view frames are arriving"));
}

#[test]
fn diagnosis_says_no_camera_when_unplugged() {
    let mut r = rig();
    r.unplugged.store(true, Ordering::Relaxed);
    r.wait_for("searching", |a| matches!(a.status, Status::Searching));
    r.app_mut().diagnose_open = true;
    r.settle(3);
    assert!(r.has_label("No camera found"));
}

#[test]
fn diagnosis_flags_live_view_paused_in_preview() {
    let mut r = rig().connected();
    r.capture_frames(1);
    r.press(Key::Tab);
    r.app_mut().diagnose_open = true;
    r.settle(3);
    assert!(r.has_label("Live view is paused while you're in Preview"));
}

// ----------------------------------------------------------------- compile

#[test]
fn compile_dialog_shows_progress_and_the_result() {
    if std::process::Command::new("ffmpeg").arg("-version").output().is_err() {
        eprintln!("ffmpeg not on PATH; skipping");
        return;
    }
    let mut r = rig().connected();
    r.capture_frames(3);
    r.app_mut().compile.open = true;
    r.settle(2);
    r.click("Compile");
    r.wait_for("compile finished", |a| a.compile.result.is_some());
    match r.app().compile.result.as_ref().unwrap() {
        Ok(CompileDone::One(out)) => {
            assert_eq!(out.frames, 3);
            assert!(out.path.is_file());
        }
        Ok(CompileDone::Each(_)) => panic!("whole-project compile produced a folder"),
        Err(e) => panic!("compile failed: {e}"),
    }
    assert!(r.has_label("Saved 3 frames to"));
}

fn have_ffmpeg() -> bool {
    let ok = std::process::Command::new("ffmpeg").arg("-version").output().is_ok();
    if !ok {
        eprintln!("ffmpeg not on PATH; skipping");
    }
    ok
}

#[test]
fn compile_for_edit_writes_one_named_file_per_scene_into_a_folder() {
    if !have_ffmpeg() {
        return;
    }
    let mut r = rig().connected();
    r.capture_frames(2);
    r.click("Add scene");
    r.capture_frames(1);
    r.app_mut().open_compile(Some(Scope::Each));
    r.settle(2);
    assert_eq!(r.app().compile.format, Format::ProRes, "compile for edit defaults to ProRes");
    assert!(r.has_label("A new folder in exports/"), "for-edit hint shown");
    r.click("Compile");
    r.wait_for("compile finished", |a| a.compile.result.is_some());
    match r.app().compile.result.as_ref().unwrap() {
        Ok(CompileDone::Each(out)) => {
            assert!(out.dir.starts_with(r.root.join("exports")));
            let names: Vec<String> =
                out.files.iter().map(|f| f.path.file_name().unwrap().to_string_lossy().into_owned()).collect();
            assert_eq!(names.len(), 2, "{names:?}");
            assert!(names[0].starts_with("01 ") && names[0].ends_with(".mov"), "{names:?}");
            assert!(names[1].starts_with("02 ") && names[1].ends_with(".mov"), "{names:?}");
            assert_eq!(out.files.iter().map(|f| f.frames).collect::<Vec<_>>(), [2, 1]);
            assert!(out.files.iter().all(|f| f.path.is_file()));
        }
        Ok(CompileDone::One(_)) => panic!("compile for edit produced a single file"),
        Err(e) => panic!("compile failed: {e}"),
    }
    assert!(r.has_label("Saved 2 scenes to"));
}

#[test]
fn compile_can_target_a_scene_other_than_the_active_one() {
    if !have_ffmpeg() {
        return;
    }
    let mut r = rig().connected();
    r.capture_frames(2);
    let first = r.app().scenes[0].id.clone();
    r.click("Add scene");
    r.capture_frames(1);
    r.app_mut().open_compile(None);
    {
        let d = &mut r.app_mut().compile;
        d.scope = Scope::Scene;
        d.scene = Some(first.clone());
    }
    r.settle(2);
    r.click("Compile");
    r.wait_for("compile finished", |a| a.compile.result.is_some());
    match r.app().compile.result.as_ref().unwrap() {
        Ok(CompileDone::One(out)) => {
            assert_eq!(out.frames, 2, "compiled the first scene, not the active second one");
            assert!(out.path.file_name().unwrap().to_string_lossy().contains(&first));
        }
        other => panic!("unexpected result: {:?}", other.as_ref().err()),
    }
}

#[test]
fn opening_compile_defaults_the_scene_choice_to_the_active_scene() {
    let mut r = rig().connected();
    r.click("Add scene");
    let active = r.app().active_row().map(|row| row.id.clone());
    r.app_mut().open_compile(None);
    assert_eq!(r.app().compile.scene, active);
}

// -------------------------------------------------- resume & crash recovery

#[test]
fn welcome_offers_to_continue_the_last_project() {
    let mut r = rig_with(false, |_| {});
    let root = r.root.clone();
    {
        let a = r.app_mut();
        a.welcome_recent = vec![RecentProject::describe(&root)];
        a.last_session_crashed = false;
    }
    r.settle(2);
    assert!(r.has_label("You were working on"));
    assert!(!r.has_label("didn't close properly"));
    r.click("Continue where you left off");
    assert_eq!(r.app().project.as_ref().unwrap().name(), "Film");
}

#[test]
fn welcome_explains_an_unclean_exit() {
    let mut r = rig_with(false, |_| {});
    let root = r.root.clone();
    {
        let a = r.app_mut();
        a.welcome_recent = vec![RecentProject::describe(&root)];
        a.last_session_crashed = true;
    }
    r.settle(2);
    assert!(r.has_label("didn't close properly"));
}

#[test]
fn missing_recent_projects_are_listed_but_not_offered() {
    let mut r = rig_with(false, |_| {});
    let gone = r._tmp.path().join("Deleted Film");
    r.app_mut().welcome_recent = vec![RecentProject::describe(&gone)];
    r.settle(2);
    assert!(!r.has_label("Continue where you left off"));
    assert!(r.has_label("not found"));
}

#[test]
fn opening_a_project_after_a_crash_recovers_the_interrupted_frame() {
    let mut r = rig_with(true, |root| {
        // A capture that crashed after downloading: pending journal entry + files in incoming/.
        let p = Project::open(root).unwrap();
        let scene = p.active_scene().unwrap();
        let _ = dragonslayer_core::capture::capture(&p, None, |dir| {
            let f = dir.join("IMG_0001.JPG");
            fs::write(&f, [0xFF, 0xD8, 0xFF, 0xD9]).unwrap();
            Err("simulated crash".into())
        });
        assert_eq!(scene.pending().unwrap().len(), 1);
    });
    r.settle(2);
    assert_eq!(r.frames(), 1);
    assert!(r.message().contains("Recovered 1 interrupted"), "{}", r.message());
}

// ------------------------------------------------------------ pure helpers

#[test]
fn human_secs_formats_minutes() {
    assert_eq!(human_secs(0.4), "0s");
    assert_eq!(human_secs(59.4), "59s");
    assert_eq!(human_secs(61.0), "1m 01s");
    assert_eq!(human_secs(3599.0), "59m 59s");
}

#[test]
fn fit_keeps_aspect_and_stays_inside() {
    let area = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(400.0, 300.0));
    let r = fit(area, egui::vec2(1920.0, 1080.0));
    assert!((r.width() / r.height() - 16.0 / 9.0).abs() < 1e-3);
    assert!(area.contains_rect(r.shrink(0.01)));
    assert_eq!(fit(area, egui::vec2(0.0, 10.0)), area, "degenerate size falls back to the area");
}

#[test]
fn cover_uv_crops_the_overhang_symmetrically() {
    let area = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(400.0, 400.0));
    let uv = cover_uv(area, egui::vec2(800.0, 400.0));
    assert!((uv.width() - 0.5).abs() < 1e-4 && (uv.height() - 1.0).abs() < 1e-4);
    assert!((uv.center().x - 0.5).abs() < 1e-4);
    let full = cover_uv(area, egui::vec2(0.0, 0.0));
    assert_eq!(full, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)));
}

/// DS_PROJECT=<real project> cargo test -p dragonslayer-app --release -- --ignored --nocapture real_project_preview
#[test]
#[ignore = "needs DS_PROJECT pointing at a real project with camera JPEGs"]
fn real_project_preview() {
    let Ok(root) = std::env::var("DS_PROJECT") else { return };
    let root = PathBuf::from(root);
    let backend = MockBackend::default();
    let project = Some(root.clone());
    let mut h = Harness::builder()
        .with_size(egui::vec2(1400.0, 900.0))
        .build_eframe(move |cc| DragonSlayerApp::new(cc, Some(Box::new(backend)), project));
    let t0 = Instant::now();
    while !h.state().camera_ready() && t0.elapsed() < WAIT {
        h.step();
        std::thread::sleep(Duration::from_millis(10));
    }
    let n = h.state().frames.len();
    println!("{n} frames, live width {}", h.state().onion_width());
    h.state_mut().jump_to(Some(0));
    let mut report = Vec::new();
    for i in 0..n.min(30) {
        h.state_mut().jump_to(Some(i));
        let t = Instant::now();
        let want = h.state().frames[i].jpeg().unwrap().to_string_lossy().into_owned();
        loop {
            h.step();
            let ok = h.state().preview_hold.as_ref().is_some_and(|t| t.name().contains(&want));
            if ok {
                report.push((i, t.elapsed(), true));
                break;
            }
            if t.elapsed() > Duration::from_secs(10) {
                report.push((i, t.elapsed(), false));
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    for (i, d, ok) in &report {
        println!("frame {i:>3}: {} after {d:?}", if *ok { "shown" } else { "STUCK" });
    }
    let queued = h.state().images.debug_state();
    println!("image cache: {queued}");
}

// ------------------------------------------------------------- RAW-only frames

#[test]
fn raw_only_formats_are_recognised() {
    use super::tabs::is_raw_only;
    for raw in ["RAW", "raw", "RAW 2", "cRAW"] {
        assert!(is_raw_only(raw), "{raw}");
    }
    for ok in ["RAW + Large Fine JPEG", "RAW+Fine", "Large Fine JPEG", "Fine", "Standard", "RAW + JPG"] {
        assert!(!is_raw_only(ok), "{ok}");
    }
}

#[test]
fn a_raw_only_frame_says_why_it_cant_be_shown_instead_of_loading_forever() {
    // Found with a real 100D set to RAW: Preview sat on "Loading…" for .CR2-only frames.
    let mut r = rig_with(true, |root| {
        let p = Project::open(root).unwrap();
        dragonslayer_core::capture::capture(&p, Some("Canon EOS 100D"), |dir| {
            let f = dir.join("IMG_0001.CR2");
            fs::write(&f, b"raw").unwrap();
            Ok(vec![f])
        })
        .unwrap();
    });
    r.settle(2);
    r.press(Key::Tab);
    r.settle(3);
    assert!(r.has_label("is RAW only"), "the viewer should explain RAW-only frames");
    assert!(!r.has_label("Loading…"));
    let f = r.app().frames[0].clone();
    assert!(r.app().frame_problem(&f, 640).is_some());
}

#[test]
fn capturing_a_raw_only_frame_warns_immediately() {
    let mut r = rig().connected();
    r.wait_for("settings", |a| !a.camera_settings.is_empty());
    let _ = r.app().session.cmd.send(Cmd::SetSetting(dragonslayer_camera::SettingKind::ImageFormat, "RAW".into()));
    r.wait_for("format RAW", |a| {
        a.camera_settings.iter().any(|s| s.kind == dragonslayer_camera::SettingKind::ImageFormat && s.value == "RAW")
    });
    r.capture_frames(1);
    assert!(r.message().contains("RAW only"), "{}", r.message());
    assert!(r.app().message.as_ref().unwrap().1, "shown as a warning");
}

#[test]
fn a_damaged_jpeg_says_so_instead_of_loading_forever() {
    let mut r = rig_with(true, |root| {
        let p = Project::open(root).unwrap();
        dragonslayer_core::capture::capture(&p, None, |dir| {
            let f = dir.join("IMG_0001.JPG");
            fs::write(&f, [0xFF, 0xD8, 0xFF, 0xE0, 0, 0]).unwrap();
            Ok(vec![f])
        })
        .unwrap();
    });
    r.press(Key::Tab);
    r.wait_for("decode to fail", |a| {
        let f = &a.frames[0];
        a.frame_problem(f, a.onion_width()).is_some()
    });
    r.settle(2);
    assert!(r.has_label("Couldn't read frame 000001"));
}

/// Renders screens to PNGs for eyeballing: DS_SHOTS=<dir> cargo test -p dragonslayer-app -- --ignored --nocapture screenshots
#[test]
#[ignore = "writes PNGs for a human to look at"]
fn screenshots() {
    let Ok(dir) = std::env::var("DS_SHOTS") else { return };
    let dir = PathBuf::from(dir);
    fs::create_dir_all(&dir).unwrap();
    for theme in [crate::theme::ThemeChoice::DarkTeal, crate::theme::ThemeChoice::DarkAmber, crate::theme::ThemeChoice::Light] {
        let mut r = rig().connected();
        crate::theme::install(&r.h.ctx, theme);
        r.capture_frames(2);
        r.wait_for("settings", |a| !a.camera_settings.is_empty());
        r.wait_for("live", |a| a.last_live_at.is_some());
        r.app_mut().diagnose_open = true;
        r.settle(5);
        let name = format!("{theme:?}").to_lowercase();
        match r.h.render() {
            Ok(img) => {
                let p = dir.join(format!("diagnose-{name}.png"));
                img.save(&p).unwrap();
                println!("wrote {}", p.display());
            }
            Err(e) => println!("render failed: {e}"),
        }
    }
}

#[test]
fn every_listed_project_opens_with_a_click() {
    let mut r = rig_with(false, |_| {});
    let other = r._tmp.path().join("Second Film");
    Project::create(&other, "Second Film", 12).unwrap();
    let first = r.root.clone();
    r.app_mut().welcome_recent = vec![RecentProject::describe(&first), RecentProject::describe(&other)];
    r.settle(2);
    // The second one is in the list (the first is the "continue" card).
    r.click("Second Film");
    assert_eq!(r.app().project.as_ref().unwrap().name(), "Second Film");
}

#[test]
#[ignore = "writes a PNG for a human to look at"]
fn screenshot_welcome() {
    let Ok(dir) = std::env::var("DS_SHOTS") else { return };
    let mut r = rig_with(false, |_| {});
    crate::theme::install(&r.h.ctx, crate::theme::ThemeChoice::DarkTeal);
    let mut list = vec![RecentProject::describe(&r.root)];
    for name in ["Hannah2", "My Film2", "Chase scene"] {
        let p = r._tmp.path().join(name);
        Project::create(&p, name, 12).unwrap();
        list.push(RecentProject::describe(&p));
    }
    list.push(RecentProject::describe(&r._tmp.path().join("On a USB stick")));
    r.app_mut().welcome_recent = list;
    r.settle(3);
    let img = r.h.render().unwrap();
    let p = PathBuf::from(dir).join("welcome.png");
    img.save(&p).unwrap();
    println!("wrote {}", p.display());
}
