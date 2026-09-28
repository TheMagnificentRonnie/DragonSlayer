//! End-to-end tests: run the real `dragonslayer` binary against the mock camera.

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, SystemTime};

const JPEG: &[u8] = &[0xFF, 0xD8, 0xFF, 0xE0, 1, 2, 3, 4, 0xFF, 0xD9];
const CUT_JPEG: &[u8] = &[0xFF, 0xD8, 0xFF, 0xE0, 1, 2, 3, 4];

fn run<I, S>(args: I) -> Output
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    Command::new(env!("CARGO_BIN_EXE_dragonslayer")).args(args).output().expect("run dragonslayer")
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

#[track_caller]
fn ok(o: Output) -> Output {
    assert!(o.status.success(), "expected success\nstdout:\n{}\nstderr:\n{}", stdout(&o), stderr(&o));
    o
}

#[track_caller]
fn fails(o: Output, needle: &str) -> Output {
    assert!(!o.status.success(), "expected failure\nstdout:\n{}\nstderr:\n{}", stdout(&o), stderr(&o));
    let err = stderr(&o);
    assert!(err.contains(needle), "stderr should mention {needle:?}:\n{err}");
    o
}

fn p(path: &Path) -> &str {
    path.to_str().unwrap()
}

/// A new project at `<tmp>/Film` (12 fps, one scene sc010 "Scene 1").
fn new_project(tmp: &Path) -> PathBuf {
    let dir = tmp.join("Film");
    ok(run(["new", p(&dir)]));
    dir
}

fn list(project: &Path) -> String {
    stdout(&ok(run(["scene", "list", p(project)])))
}

fn write_at(path: &Path, bytes: &[u8], minute: u64) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
    let t = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000 + minute * 60);
    File::options().write(true).open(path).unwrap().set_modified(t).unwrap();
}

fn files_in(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir)
        .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default();
    v.sort();
    v
}

// ------------------------------------------------------------------ new

#[test]
fn new_uses_folder_name_and_default_fps() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("My Film");
    let o = ok(run(["new", p(&dir)]));
    assert!(stdout(&o).contains("\"My Film\""), "{}", stdout(&o));
    assert!(stdout(&o).contains("12 fps"));
    assert!(dir.join("project.json").is_file());
    let l = list(&dir);
    assert!(l.starts_with("My Film (12 fps)"), "{l}");
    assert!(l.contains("* ") && l.contains("sc010") && l.contains("Scene 1"), "{l}");
}

#[test]
fn new_with_name_and_fps() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("f");
    ok(run(["new", p(&dir), "--name", "Dragon Chase", "--fps", "24"]));
    assert!(list(&dir).starts_with("Dragon Chase (24 fps)"));
}

#[test]
fn new_refuses_to_overwrite_an_existing_project() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = new_project(tmp.path());
    fails(run(["new", p(&dir), "--name", "Other"]), "already exists");
    assert!(list(&dir).starts_with("Film (12 fps)"), "original untouched");
}

#[test]
fn opening_a_folder_that_isnt_a_project_fails_clearly() {
    let tmp = tempfile::tempdir().unwrap();
    fails(run(["scene", "list", p(tmp.path())]), "opening project");
}

#[test]
fn bad_arguments_are_rejected_by_the_parser() {
    fails(run(["new"]), "Usage");
    fails(run(["frobnicate"]), "unrecognized subcommand");
    let o = ok(run(["--version"]));
    assert!(stdout(&o).starts_with("dragonslayer "));
}

// ---------------------------------------------------------------- scenes

#[test]
fn scene_add_appends_and_after_inserts_between() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = new_project(tmp.path());
    let o = ok(run(["scene", "add", p(&dir), "Chase"]));
    assert!(stdout(&o).contains("Added sc020 \"Chase\" (active)"), "{}", stdout(&o));
    let o = ok(run(["scene", "add", p(&dir), "Middle", "--after", "sc010"]));
    assert!(stdout(&o).contains("sc015"), "{}", stdout(&o));
    // --after by display name too.
    let o = ok(run(["scene", "add", p(&dir), "End", "--after", "Chase"]));
    assert!(stdout(&o).contains("sc030"), "{}", stdout(&o));

    let l = list(&dir);
    let ids: Vec<&str> = l.lines().skip(1).map(|line| line.split_whitespace().nth(if line.starts_with('*') { 2 } else { 1 }).unwrap()).collect();
    assert_eq!(ids, ["sc010", "sc015", "sc020", "sc030"], "{l}");
    assert!(l.lines().any(|line| line.starts_with('*') && line.contains("sc030")), "last added is active:\n{l}");
}

#[test]
fn scene_add_after_unknown_scene_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = new_project(tmp.path());
    fails(run(["scene", "add", p(&dir), "X", "--after", "nope"]), "not found");
}

#[test]
fn scene_rename_by_id_and_by_name() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = new_project(tmp.path());
    let o = ok(run(["scene", "rename", p(&dir), "sc010", "Opening"]));
    assert!(stdout(&o).contains("Renamed sc010 to \"Opening\""));
    ok(run(["scene", "rename", p(&dir), "Opening", "Titles"]));
    let l = list(&dir);
    assert!(l.contains("Titles") && !l.contains("Opening"), "{l}");
    fails(run(["scene", "rename", p(&dir), "Opening", "Again"]), "not found");
}

#[test]
fn scene_move_is_one_based_and_clamps() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = new_project(tmp.path());
    ok(run(["scene", "add", p(&dir), "B"]));
    ok(run(["scene", "add", p(&dir), "C"]));
    let o = ok(run(["scene", "move", p(&dir), "C", "1"]));
    assert!(stdout(&o).contains("Moved sc030 to position 1"), "{}", stdout(&o));
    let o = ok(run(["scene", "move", p(&dir), "sc030", "99"]));
    assert!(stdout(&o).contains("to position 3"), "{}", stdout(&o));
    // Position 0 behaves like 1.
    let o = ok(run(["scene", "move", p(&dir), "sc020", "0"]));
    assert!(stdout(&o).contains("to position 1"), "{}", stdout(&o));
    let l = list(&dir);
    let order: Vec<usize> = ["sc020", "sc010", "sc030"].iter().map(|id| l.find(id).unwrap()).collect();
    assert!(order.windows(2).all(|w| w[0] < w[1]), "{l}");
}

#[test]
fn scene_delete_moves_folder_to_trash() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = new_project(tmp.path());
    ok(run(["scene", "add", p(&dir), "Doomed"]));
    let o = ok(run(["scene", "delete", p(&dir), "Doomed"]));
    assert!(stdout(&o).contains("Moved scene sc020 to the project trash"));
    assert!(dir.join("trash/sc020/scene.json").is_file());
    assert!(!list(&dir).contains("Doomed"));
    fails(run(["scene", "delete", p(&dir), "Doomed"]), "not found");
}

#[test]
fn scene_fps_number_project_and_invalid() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = new_project(tmp.path());
    let o = ok(run(["scene", "fps", p(&dir), "sc010", "6"]));
    assert!(stdout(&o).contains("sc010: 6 fps"));
    assert!(list(&dir).contains("6 fps)"), "{}", list(&dir));
    let o = ok(run(["scene", "fps", p(&dir), "Scene 1", "project"]));
    assert!(stdout(&o).contains("uses project fps"));
    assert!(!list(&dir).contains(", 6 fps"));
    fails(run(["scene", "fps", p(&dir), "sc010", "fast"]), "fps must be a number");
    fails(run(["scene", "fps", p(&dir), "sc999", "6"]), "not found");
}

// --------------------------------------------------------------- cameras

#[test]
fn cameras_lists_the_mock() {
    let o = ok(run(["--mock", "cameras"]));
    let out = stdout(&o);
    assert!(out.contains("Mock Camera") && out.contains("[mock:0]"), "{out}");
    assert!(out.contains("live view: yes  capture: yes  download: yes"), "{out}");
}

#[test]
fn settings_list_one_set_and_errors() {
    let out = stdout(&ok(run(["--mock", "settings"])));
    for label in ["Aperture", "Shutter", "ISO", "White balance", "Image format"] {
        assert!(out.contains(label), "missing {label}:\n{out}");
    }
    assert!(out.contains("read-only in this mode"), "{out}");

    let out = stdout(&ok(run(["--mock", "settings", "shutter"])));
    assert!(out.contains("Shutter: 1/60") && out.contains("choices: 1/250"), "{out}");

    let out = stdout(&ok(run(["--mock", "settings", "iso", "800"])));
    assert_eq!(out.trim(), "ISO: 800");

    fails(run(["--mock", "settings", "aperture", "f/1.0"]), "not a valid Aperture");
    fails(run(["--mock", "settings", "wb", "Tungsten"]), "read-only");
    fails(run(["--mock", "settings", "iso", "800", "--camera", "usb:9,9"]), "no camera found");
    fails(run(["--mock", "settings", "exposure"]), "invalid value");
}

#[test]
fn diagnose_reports_the_mock_camera_healthy() {
    let out = stdout(&ok(run(["--mock", "diagnose"])));
    assert!(out.contains("USB"), "{out}");
    let lib = out.split("Camera library").nth(1).expect("camera library section");
    assert!(lib.contains("ok found DragonSlayer Mock Camera [mock:0]"), "{out}");
    assert!(lib.contains("ok answers commands (6 settings read)"), "{out}");
    assert!(lib.contains("ok live view frames arriving"), "{out}");
    assert!(!lib.contains(" x "), "{out}");
}

// --------------------------------------------------------------- capture

#[test]
fn capture_counts_frames_and_makes_scene_active() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = new_project(tmp.path());
    ok(run(["scene", "add", p(&dir), "Chase"]));
    let o = ok(run(["--mock", "capture", p(&dir), "Scene 1", "--count", "3", "--interval", "0.05"]));
    let out = stdout(&o);
    for n in 1..=3 {
        assert!(out.contains(&format!("sc010: frame 00000{n} (2 files)")), "{out}");
    }
    assert!(!stderr(&o).contains("no RAW"), "mock gives RAW+JPEG");
    let l = list(&dir);
    assert!(l.lines().any(|line| line.starts_with('*') && line.contains("sc010") && line.contains("(3 frames")), "{l}");
    let frames = files_in(&dir.join("scenes/sc010/frames"));
    assert_eq!(frames, ["000001.RAW", "000001.jpg", "000002.RAW", "000002.jpg", "000003.RAW", "000003.jpg"]);
}

#[test]
fn capture_into_unknown_scene_or_missing_camera_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = new_project(tmp.path());
    fails(run(["--mock", "capture", p(&dir), "Nope"]), "not found");
    fails(run(["--mock", "capture", p(&dir), "sc010", "--camera", "usb:1,2"]), "no camera found");
    assert!(list(&dir).contains("(0 frames"));
}

#[test]
fn delete_last_moves_to_trash_and_errors_on_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = new_project(tmp.path());
    fails(run(["delete-last", p(&dir), "sc010"]), "has no frames");
    ok(run(["--mock", "capture", p(&dir), "sc010", "--count", "2"]));
    let o = ok(run(["delete-last", p(&dir), "Scene 1"]));
    assert!(stdout(&o).contains("Moved frame 000002 of Scene 1 to trash (1 left)"), "{}", stdout(&o));
    assert_eq!(files_in(&dir.join("scenes/sc010/trash")), ["000002.RAW", "000002.jpg"]);
    // Numbers aren't reused.
    let o = ok(run(["--mock", "capture", p(&dir), "sc010"]));
    assert!(stdout(&o).contains("frame 000003"), "{}", stdout(&o));
}

// ---------------------------------------------------------------- import

/// Card layout. Taken order: 0421 (+CR2), 0422 (cut short), 0001, 0002.
fn card(root: &Path) -> PathBuf {
    let dcim = root.join("DCIM");
    write_at(&dcim.join("100CANON/IMG_0421.JPG"), JPEG, 1);
    write_at(&dcim.join("100CANON/IMG_0421.CR2"), b"raw", 1);
    write_at(&dcim.join("100CANON/IMG_0422.JPG"), CUT_JPEG, 2);
    write_at(&dcim.join("100CANON/IMG_0001.JPG"), JPEG, 3);
    write_at(&dcim.join("100CANON/IMG_0002.JPG"), JPEG, 4);
    write_at(&dcim.join("100CANON/MVI_0500.MOV"), b"video", 5);
    write_at(&dcim.join("MISC/notes.txt"), b"hi", 6);
    dcim
}

fn dry_run_sources(out: &str) -> Vec<String> {
    out.lines().filter(|l| l.starts_with("  ")).map(|l| l.split_whitespace().next().unwrap().to_owned()).collect()
}

#[test]
fn import_dry_run_lists_shots_in_taken_or_name_order() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = new_project(tmp.path());
    let dcim = card(tmp.path());
    let out = stdout(&ok(run(["import", p(&dir), p(&dcim), "--dry-run"])));
    assert!(out.contains("Found 4 shots (0 RAW only), 0 already in the scene, 2 other files ignored"), "{out}");
    assert_eq!(
        dry_run_sources(&out),
        ["100CANON/IMG_0421", "100CANON/IMG_0422", "100CANON/IMG_0001", "100CANON/IMG_0002"]
    );
    assert!(out.contains("100CANON/IMG_0421  (2 files)"), "{out}");

    let out = stdout(&ok(run(["import", p(&dir), p(&dcim), "--dry-run", "--order", "name"])));
    assert_eq!(
        dry_run_sources(&out),
        ["100CANON/IMG_0001", "100CANON/IMG_0002", "100CANON/IMG_0421", "100CANON/IMG_0422"]
    );
    // Dry run copies nothing and adds no scene.
    assert!(!list(&dir).contains("Imported"));
}

#[test]
fn import_into_new_scene_pairs_raw_flags_cut_jpeg_and_leaves_card_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = new_project(tmp.path());
    let dcim = card(tmp.path());
    let before: Vec<Vec<u8>> = ["IMG_0421.JPG", "IMG_0421.CR2", "IMG_0422.JPG"]
        .iter()
        .map(|f| fs::read(dcim.join("100CANON").join(f)).unwrap())
        .collect();

    let o = ok(run(["import", p(&dir), p(&dcim), "--new-scene", "Rescued"]));
    let out = stdout(&o);
    assert!(out.contains("Imported 4 frames into sc020 \"Rescued\""), "{out}");
    assert!(stderr(&o).contains("frame 000002 (100CANON/IMG_0422) looks cut short"), "{}", stderr(&o));

    let frames = files_in(&dir.join("scenes/sc020/frames"));
    assert_eq!(frames, ["000001.CR2", "000001.jpg", "000002.jpg", "000003.jpg", "000004.jpg"]);
    let after: Vec<Vec<u8>> = ["IMG_0421.JPG", "IMG_0421.CR2", "IMG_0422.JPG"]
        .iter()
        .map(|f| fs::read(dcim.join("100CANON").join(f)).unwrap())
        .collect();
    assert_eq!(before, after, "card untouched");
    assert!(list(&dir).contains("Rescued  (4 frames"), "{}", list(&dir));
}

#[test]
fn import_default_scene_name_and_rerun_skips_duplicates() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = new_project(tmp.path());
    let dcim = card(tmp.path());
    ok(run(["import", p(&dir), p(&dcim)]));
    assert!(list(&dir).contains("Imported  (4 frames"), "{}", list(&dir));

    let out = stdout(&ok(run(["import", p(&dir), p(&dcim), "--scene", "Imported"])));
    assert!(out.contains("Found 0 shots (0 RAW only), 4 already in the scene"), "{out}");
    assert!(out.contains("Nothing to import."), "{out}");

    write_at(&dcim.join("100CANON/IMG_0003.JPG"), JPEG, 9);
    let out = stdout(&ok(run(["import", p(&dir), p(&dcim), "--scene", "sc020"])));
    assert!(out.contains("Imported 1 frames into sc020"), "{out}");
    assert!(list(&dir).contains("(5 frames"), "{}", list(&dir));
}

#[test]
fn import_into_existing_scene_appends_after_captured_frames() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = new_project(tmp.path());
    ok(run(["--mock", "capture", p(&dir), "sc010", "--count", "2"]));
    let dcim = card(tmp.path());
    let out = stdout(&ok(run(["import", p(&dir), p(&dcim), "--scene", "Scene 1"])));
    assert!(out.contains("Imported 4 frames into sc010"), "{out}");
    assert!(dir.join("scenes/sc010/frames/000003.CR2").is_file());
    assert!(dir.join("scenes/sc010/frames/000006.jpg").is_file());
    assert!(!list(&dir).contains("Imported"), "no new scene");
}

#[test]
fn import_raw_only_shots_are_kept_and_reported() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = new_project(tmp.path());
    let card = tmp.path().join("card");
    write_at(&card.join("DSC_0001.NEF"), b"raw", 1);
    write_at(&card.join("DSC_0002.JPG"), JPEG, 2);
    let out = stdout(&ok(run(["import", p(&dir), p(&card)])));
    assert!(out.contains("Found 2 shots (1 RAW only)"), "{out}");
    assert!(out.contains("1 shots are RAW only"), "{out}");
    assert_eq!(files_in(&dir.join("scenes/sc020/frames")), ["000001.NEF", "000002.jpg"]);
}

#[test]
fn import_individual_files() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = new_project(tmp.path());
    let dcim = card(tmp.path());
    let a = dcim.join("100CANON/IMG_0001.JPG");
    let b = dcim.join("100CANON/IMG_0002.JPG");
    let out = stdout(&ok(run(["import", p(&dir), p(&a), p(&b)])));
    assert!(out.contains("Imported 2 frames"), "{out}");
}

#[test]
fn import_errors_nonexistent_source_conflicts_and_unknown_scene() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = new_project(tmp.path());
    let missing = tmp.path().join("no-such-card");
    let o = ok(run(["import", p(&dir), p(&missing)]));
    assert!(stderr(&o).contains("couldn't read"), "{}", stderr(&o));
    assert!(stdout(&o).contains("Nothing to import."), "{}", stdout(&o));

    let dcim = card(tmp.path());
    fails(run(["import", p(&dir), p(&dcim), "--scene", "sc010", "--new-scene", "X"]), "cannot be used with");
    fails(run(["import", p(&dir), p(&dcim), "--scene", "nope"]), "not found");
    fails(run(["import", p(&dir)]), "required");
    assert!(!list(&dir).contains("Imported"), "failed imports add no scene");
}

// --------------------------------------------------------------- compile

fn have_ffmpeg() -> bool {
    Command::new("ffmpeg").arg("-version").output().is_ok_and(|o| o.status.success())
}

#[test]
fn compile_empty_project_fails_without_needing_ffmpeg() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = new_project(tmp.path());
    fails(run(["compile", p(&dir)]), "nothing to compile");
    fails(run(["compile", p(&dir), "Scene 1"]), "nothing to compile");
    fails(run(["compile", p(&dir), "nope"]), "not found");
}

#[test]
fn compile_mp4_mov_scene_and_fps_override() {
    if !have_ffmpeg() {
        eprintln!("note: ffmpeg not on PATH; skipping compile test");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let dir = new_project(tmp.path());
    ok(run(["--mock", "capture", p(&dir), "sc010", "--count", "3"]));
    ok(run(["scene", "add", p(&dir), "Empty"]));
    let exports = dir.join("exports");

    let o = ok(run(["compile", p(&dir), "--resolution", "1080p"]));
    let err = stderr(&o);
    assert!(err.contains("Compiling… 100%"), "{err}");
    assert!(err.contains("warning: scene \"Empty\" (sc020) is empty; skipped"), "{err}");
    assert!(stdout(&o).contains("(3 frames)"), "{}", stdout(&o));
    let mp4: Vec<String> = files_in(&exports).into_iter().filter(|f| f.ends_with(".mp4")).collect();
    assert_eq!(mp4.len(), 1, "{mp4:?}");
    assert!(mp4[0].starts_with("Film_all_"), "{mp4:?}");
    assert!(fs::metadata(exports.join(&mp4[0])).unwrap().len() > 0);

    let o = ok(run(["compile", p(&dir), "Scene 1", "--format", "prores", "--fps", "24", "--crop", "--resolution", "1080p"]));
    assert!(stdout(&o).contains(".mov (3 frames)"), "{}", stdout(&o));
    let mov: Vec<String> = files_in(&exports).into_iter().filter(|f| f.ends_with(".mov")).collect();
    assert_eq!(mov.len(), 1, "{mov:?}");
    assert!(mov[0].starts_with("Film_sc010_"), "{mov:?}");
    // No stray concat list left behind.
    assert!(files_in(&exports).iter().all(|f| !f.ends_with(".txt")), "{:?}", files_in(&exports));
}

// ------------------------------------------------------------- recovery

#[test]
fn any_command_recovers_an_interrupted_capture() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = new_project(tmp.path());
    let scene = dir.join("scenes/sc010");
    fs::write(
        scene.join("journal.ndjson"),
        "{\"t\":\"2026-09-27T12:00:00Z\",\"op\":\"pending\",\"frame\":\"000001\"}\n",
    )
    .unwrap();
    fs::create_dir_all(scene.join("incoming/000001")).unwrap();
    fs::write(scene.join("incoming/000001/IMG_0001.JPG"), JPEG).unwrap();

    let o = ok(run(["scene", "list", p(&dir)]));
    assert!(stderr(&o).contains("recovered interrupted capture: sc010 frame 000001"), "{}", stderr(&o));
    assert!(stdout(&o).contains("(1 frames"), "{}", stdout(&o));
    assert!(scene.join("frames/000001.jpg").is_file());
    // Second open: nothing left to recover.
    let o = ok(run(["scene", "list", p(&dir)]));
    assert!(!stderr(&o).contains("recovered"), "{}", stderr(&o));
}

#[test]
fn interrupted_capture_with_no_files_is_reported_as_abandoned() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = new_project(tmp.path());
    fs::write(
        dir.join("scenes/sc010/journal.ndjson"),
        "{\"t\":\"2026-09-27T12:00:00Z\",\"op\":\"pending\",\"frame\":\"000001\"}\n",
    )
    .unwrap();
    let o = ok(run(["scene", "list", p(&dir)]));
    assert!(stderr(&o).contains("had no files; it may still be on the camera card"), "{}", stderr(&o));
    let o = ok(run(["--mock", "capture", p(&dir), "sc010"]));
    assert!(stdout(&o).contains("frame 000002"), "abandoned number isn't reused: {}", stdout(&o));
}

#[test]
fn compile_frames_argument_is_checked() {
    let tmp = tempfile::tempdir().unwrap();
    let proj = tmp.path().join("Film");
    let p = proj.to_str().unwrap();
    assert!(run(["--mock", "new", p]).status.success());
    assert!(run(["--mock", "capture", p, "Scene 1", "--count", "3"]).status.success());
    for bad in ["0-2", "3-1", "x-2", "5"] {
        let out = run(["compile", p, "sc010", "--frames", bad]);
        assert!(!out.status.success(), "--frames {bad} should be refused");
    }
    // A range needs a scene.
    assert!(!run(["compile", p, "--frames", "1-2"]).status.success());

    if std::process::Command::new("ffmpeg").arg("-version").output().is_err() {
        eprintln!("ffmpeg not on PATH; skipping the compile itself");
        return;
    }
    let out = run(["compile", p, "sc010", "--frames", "2-3"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("(2 frames)"), "{stdout}");
    assert!(stdout.contains("sc010_f2-3"), "file named after the range: {stdout}");
}

#[test]
fn takes_from_the_command_line() {
    let tmp = tempfile::tempdir().unwrap();
    let proj = tmp.path().join("Film");
    let p = proj.to_str().unwrap();
    assert!(run(["--mock", "new", p]).status.success());
    assert!(run(["--mock", "capture", p, "Scene 1", "--count", "1"]).status.success());
    let out = run(["scene", "take", p, "Scene 1"]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("sc010t2"));
    assert!(run(["--mock", "capture", p, "sc010t2", "--count", "2"]).status.success());
    assert!(run(["scene", "use", p, "sc010", "2"]).status.success());
    let list = String::from_utf8_lossy(&run(["scene", "list", p]).stdout).into_owned();
    assert!(list.contains("take 2  sc010t2  (2 frames)  ★ in film"), "{list}");
    // Back to the original, by number.
    assert!(run(["scene", "use", p, "sc010", "1"]).status.success());
    let list = String::from_utf8_lossy(&run(["scene", "list", p]).stdout).into_owned();
    assert!(list.contains("Scene 1  (1 frames)  ★ in film"), "{list}");
    // Nonsense is refused.
    assert!(!run(["scene", "use", p, "sc010", "9"]).status.success());
}

#[test]
fn reference_audio_from_the_command_line() {
    let tmp = tempfile::tempdir().unwrap();
    let proj = tmp.path().join("Film");
    let p = proj.to_str().unwrap();
    assert!(run(["--mock", "new", p]).status.success());
    let song = tmp.path().join("song.wav");
    std::fs::write(&song, b"RIFF").unwrap();
    let out = run(["scene", "audio", p, "Scene 1", song.to_str().unwrap(), "--start", "1.25"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(text.contains("song.wav from 1.25 s"), "{text}");
    assert!(proj.join("audio/song.wav").exists());
    // Just the start; then removed.
    let text = String::from_utf8_lossy(&run(["scene", "audio", p, "sc010", "--start", "3"]).stdout).into_owned();
    assert!(text.contains("from 3.00 s"), "{text}");
    assert!(!run(["scene", "audio", p, "sc010", "--start", "-1"]).status.success());
    let text = String::from_utf8_lossy(&run(["scene", "audio", p, "sc010", "none"]).stdout).into_owned();
    assert!(text.contains("no reference audio"), "{text}");
}
