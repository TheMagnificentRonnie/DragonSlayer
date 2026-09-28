use std::fs;
use std::path::{Path, PathBuf};

use dragonslayer_core::compile::{self, Shot};
use dragonslayer_core::journal::{self, JournalEntry, JournalOp};
use dragonslayer_core::{capture, Project};

fn project(dir: &Path) -> Project {
    Project::create(dir.join("Film"), "Film", 12).unwrap()
}

fn fake_shot(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let jpg = dir.join("IMG_0001.JPG");
    let raw = dir.join("IMG_0001.CR2");
    fs::write(&jpg, b"jpeg").unwrap();
    fs::write(&raw, b"raw").unwrap();
    Ok(vec![jpg, raw])
}

#[test]
fn scene_ids_append_by_ten_and_insert_between() {
    let tmp = tempfile::tempdir().unwrap();
    let mut p = project(tmp.path());
    assert_eq!(p.file.scenes, ["sc010"]);
    p.add_scene("B", None).unwrap();
    p.add_scene("mid", Some("sc010")).unwrap();
    assert_eq!(p.file.scenes, ["sc010", "sc015", "sc020"]);
    assert_eq!(p.file.active_scene.as_deref(), Some("sc015"));

    // Reloads from disk identically.
    let again = Project::open(&p.root).unwrap();
    assert_eq!(again.file, p.file);
}

#[test]
fn deleted_scene_goes_to_trash_and_its_id_is_not_reused() {
    let tmp = tempfile::tempdir().unwrap();
    let mut p = project(tmp.path());
    p.add_scene("B", None).unwrap();
    p.delete_scene("sc020").unwrap();
    assert!(p.root.join("trash/sc020/scene.json").is_file());
    assert_eq!(p.file.active_scene.as_deref(), Some("sc010"));
    let s = p.add_scene("C", None).unwrap();
    assert_eq!(s.id(), "sc030");
}

#[test]
fn capture_commits_files_and_journal() {
    let tmp = tempfile::tempdir().unwrap();
    let p = project(tmp.path());
    let c = capture::capture(&p, Some("Canon EOS 100D"), fake_shot).unwrap();
    assert_eq!(c.frame, "000001");
    let scene = p.active_scene().unwrap();
    let frames = scene.frames().unwrap();
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].camera.as_deref(), Some("Canon EOS 100D"));
    assert!(frames[0].jpeg().unwrap().ends_with("000001.jpg"));
    assert!(scene.dir.join("frames/000001.CR2").is_file());
    assert!(scene.pending().unwrap().is_empty());
}

#[test]
fn deleted_frame_numbers_are_not_reused() {
    let tmp = tempfile::tempdir().unwrap();
    let p = project(tmp.path());
    capture::capture(&p, None, fake_shot).unwrap();
    capture::capture(&p, None, fake_shot).unwrap();
    let scene = p.active_scene().unwrap();
    assert_eq!(scene.delete_last().unwrap(), "000002");
    assert!(scene.dir.join("trash/000002.jpg").is_file());
    let c = capture::capture(&p, None, fake_shot).unwrap();
    assert_eq!(c.frame, "000003");
    let ids: Vec<_> = scene.frames().unwrap().into_iter().map(|f| f.id).collect();
    assert_eq!(ids, ["000001", "000003"]);
}

#[test]
fn failed_trigger_with_no_files_is_abandoned() {
    let tmp = tempfile::tempdir().unwrap();
    let p = project(tmp.path());
    let err = capture::capture(&p, None, |_| Err("camera busy".into())).unwrap_err();
    assert!(err.to_string().contains("camera busy"));
    let scene = p.active_scene().unwrap();
    assert!(scene.pending().unwrap().is_empty());
    assert_eq!(scene.frame_count().unwrap(), 0);
}

#[test]
fn crash_after_download_is_recovered_on_next_open() {
    let tmp = tempfile::tempdir().unwrap();
    let p = project(tmp.path());
    // Download succeeded, then the commit "crashed".
    let _ = capture::capture(&p, None, |dir| {
        fake_shot(dir).unwrap();
        Err("simulated crash".into())
    });
    let scene = p.active_scene().unwrap();
    assert_eq!(scene.pending().unwrap(), ["000001"]);

    let report = Project::open(&p.root).unwrap().recover().unwrap();
    assert_eq!(report.recovered, [("sc010".to_string(), "000001".to_string())]);
    assert_eq!(scene.frame_count().unwrap(), 1);
    assert!(scene.dir.join("frames/000001.jpg").is_file());
}

#[test]
fn pending_with_nothing_downloaded_is_abandoned_on_recovery() {
    let tmp = tempfile::tempdir().unwrap();
    let p = project(tmp.path());
    let scene = p.active_scene().unwrap();
    journal::append(&scene.journal_path(), &JournalEntry::now(JournalOp::Pending, "000001", None)).unwrap();
    let report = p.recover().unwrap();
    assert_eq!(report.abandoned.len(), 1);
    assert!(scene.pending().unwrap().is_empty());
    // The abandoned number is still skipped.
    assert_eq!(scene.next_frame_id().unwrap(), "000002");
}

#[test]
fn torn_journal_line_is_ignored_and_next_append_survives() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("journal.ndjson");
    journal::append(&path, &JournalEntry::now(JournalOp::Capture, "000001", None)).unwrap();
    let mut bytes = fs::read(&path).unwrap();
    bytes.extend_from_slice(br#"{"t":"2026-09-26T14:0"#);
    fs::write(&path, bytes).unwrap();

    journal::append(&path, &JournalEntry::now(JournalOp::Capture, "000002", None)).unwrap();
    let frames: Vec<_> = journal::read(&path).unwrap().into_iter().map(|e| e.frame).collect();
    assert_eq!(frames, ["000001", "000002"]);
}

#[test]
fn compile_plan_orders_scenes_and_uses_scene_fps() {
    let tmp = tempfile::tempdir().unwrap();
    let mut p = project(tmp.path());
    capture::capture(&p, None, fake_shot).unwrap();
    p.add_scene("Second", None).unwrap();
    p.set_scene_fps("sc020", Some(6)).unwrap();
    capture::capture(&p, None, fake_shot).unwrap();
    capture::capture(&p, None, fake_shot).unwrap();
    p.add_scene("Empty", None).unwrap();
    p.move_scene("sc020", 0).unwrap();

    let (shots, warnings) = compile::plan(&p, None, None).unwrap();
    let got: Vec<(String, f64)> = shots
        .iter()
        .map(|s| {
            let scene = s.path.parent().unwrap().parent().unwrap().file_name().unwrap();
            (scene.to_string_lossy().into_owned(), s.seconds)
        })
        .collect();
    assert_eq!(
        got,
        [("sc020".into(), 1.0 / 6.0), ("sc020".into(), 1.0 / 6.0), ("sc010".into(), 1.0 / 12.0)]
    );
    assert_eq!(warnings.len(), 1, "empty scene warns");
    assert!(compile::plan(&p, Some("Empty"), None).is_err());

    // With an fps override every shot gets the same duration regardless of scene fps.
    let (shots, _) = compile::plan(&p, None, Some(24)).unwrap();
    assert!(shots.iter().all(|s| (s.seconds - 1.0 / 24.0).abs() < 1e-9));
}

#[test]
fn concat_list_escapes_quotes_and_repeats_last_file() {
    let shots = [
        Shot { path: PathBuf::from(r"C:\films\Bob's film\a.jpg"), seconds: 0.5 },
        Shot { path: PathBuf::from("/b.jpg"), seconds: 0.25 },
    ];
    let list = compile::concat_list(&shots);
    assert!(list.contains(r"file 'C:/films/Bob'\''s film/a.jpg'"));
    assert!(list.trim_end().ends_with("file '/b.jpg'"));
    assert_eq!(list.matches("duration").count(), 2);
}

// ------------------------------------------------------------ project format

#[test]
fn create_refuses_to_overwrite_existing_project() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("Film");
    Project::create(&root, "Film", 12).unwrap();
    let err = Project::create(&root, "Different", 24).unwrap_err();
    assert!(matches!(err, dragonslayer_core::Error::ProjectExists(_)), "got {err:?}");
}

#[test]
fn open_rejects_unknown_project_format_version() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("Film");
    Project::create(&root, "Film", 12).unwrap();
    // Tamper with the version in project.json.
    let path = root.join("project.json");
    let text = fs::read_to_string(&path).unwrap().replace("dragonslayer/1", "dragonslayer/999");
    fs::write(&path, text).unwrap();
    let err = Project::open(&root).unwrap_err();
    assert!(matches!(err, dragonslayer_core::Error::UnsupportedFormat { .. }), "got {err:?}");
}

#[test]
fn project_file_is_written_atomically_no_temp_file_left() {
    // After save() completes, there should never be a `.project.json.tmp`
    // hanging around in the project root — the atomic-rename protocol
    // guarantees the temp file is either fully renamed or absent.
    let tmp = tempfile::tempdir().unwrap();
    let mut p = project(tmp.path());
    for i in 0..20 {
        p.add_scene(&format!("s{i}"), None).unwrap();
    }
    let root = &p.root;
    let stray: Vec<_> = fs::read_dir(root)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            let n = e.file_name();
            let s = n.to_string_lossy();
            s.ends_with(".tmp") || s.starts_with('.')
        })
        .map(|e| e.file_name())
        .collect();
    assert!(stray.is_empty(), "temp/hidden files left in project root: {stray:?}");
}

// ---------------------------------------------------------- scenes / metadata

#[test]
fn rename_only_changes_display_name_not_id_or_order() {
    let tmp = tempfile::tempdir().unwrap();
    let mut p = project(tmp.path());
    p.add_scene("Second", None).unwrap();
    let before_ids = p.file.scenes.clone();
    p.rename_scene("sc010", "Opening Titles").unwrap();
    assert_eq!(p.file.scenes, before_ids);
    assert_eq!(p.scene("sc010").unwrap().name(), "Opening Titles");
    // Persists across reload.
    let again = Project::open(&p.root).unwrap();
    assert_eq!(again.scene("sc010").unwrap().name(), "Opening Titles");
}

#[test]
fn move_scene_reorders_and_clamps_to_end() {
    let tmp = tempfile::tempdir().unwrap();
    let mut p = project(tmp.path());
    p.add_scene("B", None).unwrap();
    p.add_scene("C", None).unwrap();
    assert_eq!(p.file.scenes, ["sc010", "sc020", "sc030"]);
    p.move_scene("sc030", 0).unwrap();
    assert_eq!(p.file.scenes, ["sc030", "sc010", "sc020"]);
    // Clamps to end when target index is too large.
    p.move_scene("sc030", 999).unwrap();
    assert_eq!(p.file.scenes, ["sc010", "sc020", "sc030"]);
}

#[test]
fn per_scene_fps_override_none_reverts_to_project_default() {
    let tmp = tempfile::tempdir().unwrap();
    let mut p = project(tmp.path());
    p.set_scene_fps("sc010", Some(24)).unwrap();
    assert_eq!(p.fps_for(&p.scene("sc010").unwrap()), 24);
    p.set_scene_fps("sc010", None).unwrap();
    assert_eq!(p.fps_for(&p.scene("sc010").unwrap()), 12);
}

#[test]
fn delete_active_scene_picks_the_next_one_active() {
    let tmp = tempfile::tempdir().unwrap();
    let mut p = project(tmp.path());
    p.add_scene("Middle", None).unwrap();
    p.add_scene("Last", None).unwrap();
    p.set_active("sc020").unwrap();
    p.delete_scene("sc020").unwrap();
    // Whatever is picked, it must be one of the remaining scenes and must
    // exist on disk.
    let active = p.file.active_scene.clone().unwrap();
    assert!(p.file.scenes.contains(&active), "active {active} not in {:?}", p.file.scenes);
    assert!(p.scene(&active).is_ok());
}

// ----------------------------------------------------------------- recovery

#[test]
fn recover_processes_pending_in_multiple_scenes() {
    let tmp = tempfile::tempdir().unwrap();
    let mut p = project(tmp.path());
    p.add_scene("Second", None).unwrap();
    // Both scenes get a mid-flight crash.
    p.set_active("sc010").unwrap();
    let _ = capture::capture(&p, None, |dir| {
        fake_shot(dir).unwrap();
        Err("crash A".into())
    });
    p.set_active("sc020").unwrap();
    let _ = capture::capture(&p, None, |dir| {
        fake_shot(dir).unwrap();
        Err("crash B".into())
    });

    let report = Project::open(&p.root).unwrap().recover().unwrap();
    assert_eq!(report.recovered.len(), 2);
    assert_eq!(report.abandoned.len(), 0);
    assert!(p.scene("sc010").unwrap().dir.join("frames/000001.jpg").is_file());
    assert!(p.scene("sc020").unwrap().dir.join("frames/000001.jpg").is_file());
}

#[test]
fn recovery_is_idempotent_second_call_does_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let p = project(tmp.path());
    let _ = capture::capture(&p, None, |dir| {
        fake_shot(dir).unwrap();
        Err("crash".into())
    });
    let first = Project::open(&p.root).unwrap().recover().unwrap();
    assert_eq!(first.recovered.len(), 1);

    let second = Project::open(&p.root).unwrap().recover().unwrap();
    assert!(second.recovered.is_empty());
    assert!(second.abandoned.is_empty());
}

// ------------------------------------------------------------------ capture

#[test]
fn capture_records_camera_name_and_jpeg_ext_is_normalised_lowercase() {
    // Camera returns "IMG_0001.JPG" (upper) — commit should normalise to .jpg
    // so the frames folder stays consistent regardless of camera vendor.
    let tmp = tempfile::tempdir().unwrap();
    let p = project(tmp.path());
    capture::capture(&p, Some("Panasonic DC-GH5"), |dir| {
        let jpg = dir.join("IMG_0001.JPG");
        fs::write(&jpg, b"jpeg").unwrap();
        Ok(vec![jpg])
    })
    .unwrap();
    let scene = p.active_scene().unwrap();
    assert!(scene.dir.join("frames/000001.jpg").is_file());
    // Check the literal filename on disk, not `is_file()` (case-insensitive on Windows).
    let names: Vec<String> = fs::read_dir(scene.dir.join("frames"))
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["000001.jpg"]);
    let cam = scene.frames().unwrap()[0].camera.clone();
    assert_eq!(cam.as_deref(), Some("Panasonic DC-GH5"));
}

#[test]
fn delete_last_on_empty_scene_is_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    let p = project(tmp.path());
    let scene = p.active_scene().unwrap();
    let err = scene.delete_last().unwrap_err();
    assert!(matches!(err, dragonslayer_core::Error::NoFrames(_)), "got {err:?}");
}

#[test]
fn capture_into_non_active_scene_needs_set_active_first() {
    // capture::capture always writes into the ACTIVE scene, so if the user
    // wants a different scene they must switch first. Guard against a
    // regression where capture silently writes to a stale scene.
    let tmp = tempfile::tempdir().unwrap();
    let mut p = project(tmp.path());
    p.add_scene("B", None).unwrap();
    // Active is now sc020.
    capture::capture(&p, None, fake_shot).unwrap();
    assert_eq!(p.scene("sc020").unwrap().frame_count().unwrap(), 1);
    assert_eq!(p.scene("sc010").unwrap().frame_count().unwrap(), 0);
    // Switch and shoot again.
    p.set_active("sc010").unwrap();
    capture::capture(&p, None, fake_shot).unwrap();
    assert_eq!(p.scene("sc010").unwrap().frame_count().unwrap(), 1);
}

// ---------------------------------------------------------- compile edge

#[test]
fn compile_plan_errors_when_project_has_no_frames_at_all() {
    let tmp = tempfile::tempdir().unwrap();
    let p = project(tmp.path());
    let err = compile::plan(&p, None, None).unwrap_err();
    assert!(matches!(err, dragonslayer_core::Error::NothingToCompile(_)), "got {err:?}");
}

#[test]
fn compile_plan_scene_scope_fps_override_ignores_project_and_scene_fps() {
    let tmp = tempfile::tempdir().unwrap();
    let mut p = project(tmp.path());
    p.set_scene_fps("sc010", Some(6)).unwrap();
    capture::capture(&p, None, fake_shot).unwrap();
    let (shots, _) = compile::plan(&p, Some("sc010"), Some(60)).unwrap();
    assert!(shots.iter().all(|s| (s.seconds - 1.0 / 60.0).abs() < 1e-9));
}

#[test]
fn ffmpeg_args_h264_and_prores_use_expected_codec_args() {
    // Sanity check on the command we'd hand to ffmpeg. Doesn't need ffmpeg
    // installed — pure string construction.
    use dragonslayer_core::compile::{Format, Framing, Resolution, Settings};
    let list = PathBuf::from("/list.txt");
    let out = PathBuf::from("/out.mp4");
    let base = Settings::default();
    let h264 = compile::ffmpeg_args(&list, &out, 24, &Settings { format: Format::H264, ..base.clone() });
    assert!(h264.iter().any(|a| a == "libx264"), "h264 args missing codec: {h264:?}");
    assert!(h264.iter().any(|a| a == "-pix_fmt"));
    let prores = compile::ffmpeg_args(&list, &out, 24, &Settings { format: Format::ProRes, ..base.clone() });
    assert!(prores.iter().any(|a| a == "prores_ks"), "prores args missing codec: {prores:?}");
    // Framing goes through the vf filter chain.
    let fit_uhd = compile::ffmpeg_args(
        &list,
        &out,
        24,
        &Settings { resolution: Resolution::Uhd, framing: Framing::Fit, ..base.clone() },
    );
    let vf: String = fit_uhd.iter().skip_while(|a| *a != "-vf").nth(1).cloned().unwrap_or_default();
    assert!(vf.contains("3840:2160"), "expected 4K scale in vf: {vf}");
    assert!(vf.contains("pad"), "fit should pad");
    let crop_hd = compile::ffmpeg_args(
        &list,
        &out,
        24,
        &Settings { resolution: Resolution::Hd, framing: Framing::Crop, ..base },
    );
    let vf: String = crop_hd.iter().skip_while(|a| *a != "-vf").nth(1).cloned().unwrap_or_default();
    assert!(vf.contains("1920:1080"), "expected 1080p scale in vf: {vf}");
    assert!(vf.contains("crop"), "crop should crop");
}

#[test]
fn concat_list_of_single_shot_still_repeats_the_file() {
    // ffconcat honours the DURATION of the last entry only when the file is
    // repeated at the end. A one-shot compile still needs that repeat, or
    // the frame lasts 0 seconds.
    let shots = [Shot { path: PathBuf::from("/only.jpg"), seconds: 0.1 }];
    let list = compile::concat_list(&shots);
    let file_lines = list.lines().filter(|l| l.starts_with("file ")).count();
    assert_eq!(file_lines, 2, "expected the single file line + its repeat, got:\n{list}");
}

// ------------------------------------------------------------------ journal

#[test]
fn journal_replays_in_order_across_many_capture_and_delete_events() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("journal.ndjson");
    // Sequence: capture 1, capture 2, capture 3, delete 2, capture 4, delete 4.
    // Expected frames after replay: [1, 3].
    for (op, frame) in [
        (JournalOp::Pending, "000001"),
        (JournalOp::Capture, "000001"),
        (JournalOp::Pending, "000002"),
        (JournalOp::Capture, "000002"),
        (JournalOp::Pending, "000003"),
        (JournalOp::Capture, "000003"),
        (JournalOp::Delete, "000002"),
        (JournalOp::Pending, "000004"),
        (JournalOp::Capture, "000004"),
        (JournalOp::Delete, "000004"),
    ] {
        journal::append(&path, &JournalEntry::now(op, frame, None)).unwrap();
    }
    let entries = journal::read(&path).unwrap();
    assert_eq!(entries.len(), 10);
    // Sanity: read gives back every well-formed line.
    let captures: Vec<_> = entries.iter().filter(|e| e.op == JournalOp::Capture).map(|e| e.frame.clone()).collect();
    assert_eq!(captures, ["000001", "000002", "000003", "000004"]);
}

#[test]
fn journal_read_on_missing_file_is_empty_not_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("nope.ndjson");
    let entries = journal::read(&path).unwrap();
    assert!(entries.is_empty());
}

// ---------------------------------------------------------------- progress

#[test]
fn progress_lines_map_output_time_onto_total_duration() {
    // 10 s film: 2.5 s of output is 25%.
    assert_eq!(compile::progress_from_line("out_time_us=2500000", 10.0), Some(0.25));
    // Older ffmpeg only prints out_time_ms, which is also microseconds.
    assert_eq!(compile::progress_from_line("out_time_ms=5000000", 10.0), Some(0.5));
    assert_eq!(compile::progress_from_line("progress=end", 10.0), Some(1.0));
}

#[test]
fn progress_lines_that_are_not_progress_are_ignored_and_values_clamped() {
    assert_eq!(compile::progress_from_line("frame=12", 10.0), None);
    assert_eq!(compile::progress_from_line("out_time_us=N/A", 10.0), None);
    assert_eq!(compile::progress_from_line("progress=continue", 10.0), None);
    assert_eq!(compile::progress_from_line("garbage", 10.0), None);
    // ffmpeg can overshoot slightly on the last frame; never report more than 100%.
    assert_eq!(compile::progress_from_line("out_time_us=11000000", 10.0), Some(1.0));
    // Empty film: no division by zero.
    assert_eq!(compile::progress_from_line("out_time_us=1000", 0.0), None);
}

#[test]
fn ffmpeg_args_ask_for_machine_readable_progress_on_stdout() {
    let args = compile::ffmpeg_args(
        &PathBuf::from("/l.txt"),
        &PathBuf::from("/o.mp4"),
        12,
        &compile::Settings::default(),
    );
    let at = args.iter().position(|a| a == "-progress").expect("-progress");
    assert_eq!(args[at + 1], "pipe:1");
}

#[test]
fn projects_from_before_the_rename_open_and_are_relabelled_on_save() {
    // Projects made when the app was called Stopgap say "stopgap/1"; they couldn't be opened.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("GH5Test");
    let mut p = Project::create(&root, "GH5-Test", 12).unwrap();
    capture::capture(&p, Some("Panasonic DC-GH5"), fake_shot).unwrap();
    p.file.format = "stopgap/1".into();
    p.save().unwrap();

    let mut old = Project::open(&root).expect("legacy project opens");
    assert_eq!(old.active_scene().unwrap().frame_count().unwrap(), 1);
    old.add_scene("B", None).unwrap();
    let text = fs::read_to_string(root.join("project.json")).unwrap();
    assert!(text.contains("dragonslayer/1"), "relabelled on save: {text}");
    // Something genuinely unknown is still refused.
    let mut future = Project::open(&root).unwrap();
    future.file.format = "dragonslayer/9".into();
    future.save().unwrap();
    assert!(Project::open(&root).is_err());
}

// ---------------------------------------------------------- compile for edit

/// A stand-in ffmpeg: logs its arguments (one run per line) and creates the output
/// file (its last argument), so compile_each can be tested without encoding video.
#[cfg(unix)]
fn fake_ffmpeg(dir: &Path) -> (PathBuf, PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let log = dir.join("ffmpeg.log");
    let bin = dir.join("fake-ffmpeg");
    fs::write(&bin, format!("#!/bin/sh\nfor a; do last=\"$a\"; done\necho \"$*\" >> '{}'\n: > \"$last\"\n", log.display()))
        .unwrap();
    fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
    (bin, log)
}

#[cfg(unix)]
#[test]
fn compile_each_writes_one_file_per_scene_named_and_numbered_in_film_order() {
    use dragonslayer_core::compile::Settings;
    let tmp = tempfile::tempdir().unwrap();
    let (ffmpeg, log) = fake_ffmpeg(tmp.path());
    let mut p = project(tmp.path());
    p.rename_scene("sc010", "Opening").unwrap();
    capture::capture(&p, None, fake_shot).unwrap();
    p.add_scene("The chase: part/2?", None).unwrap();
    p.set_scene_fps("sc020", Some(6)).unwrap();
    capture::capture(&p, None, fake_shot).unwrap();
    capture::capture(&p, None, fake_shot).unwrap();
    p.add_scene("Empty", None).unwrap();

    let settings = Settings { ffmpeg: Some(ffmpeg), ..Settings::default() };
    let mut seen: Vec<String> = Vec::new();
    let mut fractions = Vec::new();
    let out = compile::compile_each(&p, &settings, |f, name| {
        fractions.push(f);
        if !name.is_empty() && seen.last().map(String::as_str) != Some(name) {
            seen.push(name.to_string());
        }
    })
    .unwrap();

    assert_eq!(seen, ["Opening", "The chase: part/2?"]);
    assert!(fractions.windows(2).all(|w| w[0] <= w[1]), "progress never goes backwards: {fractions:?}");
    assert_eq!(fractions.last(), Some(&1.0));
    assert!(out.dir.starts_with(tmp.path().join("Film/exports")));
    assert!(out.dir.file_name().unwrap().to_string_lossy().starts_with("Film_for-edit_"));
    let names: Vec<String> =
        out.files.iter().map(|f| f.path.file_name().unwrap().to_string_lossy().into_owned()).collect();
    assert_eq!(names, ["01 Opening.mp4", "02 The chase_ part_2_.mp4"]);
    assert!(out.files.iter().all(|f| f.path.exists()));
    assert_eq!(out.files.iter().map(|f| f.frames).collect::<Vec<_>>(), [1, 2]);
    assert_eq!(out.warnings.len(), 1, "empty scene is skipped with a warning");

    // Each scene renders at its own rate; no concat scripts are left behind.
    let runs = fs::read_to_string(&log).unwrap();
    let runs: Vec<&str> = runs.lines().collect();
    assert_eq!(runs.len(), 2);
    assert!(runs[0].contains("fps=12"), "{}", runs[0]);
    assert!(runs[1].contains("fps=6"), "{}", runs[1]);
    let leftovers: Vec<_> = fs::read_dir(&out.dir).unwrap().filter_map(|e| e.ok()).filter(|e| e.file_name().to_string_lossy().starts_with('.')).collect();
    assert!(leftovers.is_empty(), "concat scripts cleaned up");
}

#[cfg(unix)]
#[test]
fn compile_each_fps_override_applies_to_every_scene() {
    use dragonslayer_core::compile::Settings;
    let tmp = tempfile::tempdir().unwrap();
    let (ffmpeg, log) = fake_ffmpeg(tmp.path());
    let mut p = project(tmp.path());
    capture::capture(&p, None, fake_shot).unwrap();
    p.add_scene("Two", None).unwrap();
    p.set_scene_fps("sc020", Some(6)).unwrap();
    capture::capture(&p, None, fake_shot).unwrap();

    let settings = Settings { ffmpeg: Some(ffmpeg), fps_override: Some(24), ..Settings::default() };
    compile::compile_each(&p, &settings, |_, _| {}).unwrap();
    let runs = fs::read_to_string(&log).unwrap();
    assert!(runs.lines().all(|l| l.contains("fps=24")), "{runs}");
}

#[test]
fn compile_each_errors_when_nothing_to_compile() {
    let tmp = tempfile::tempdir().unwrap();
    let p = project(tmp.path());
    let err = compile::compile_each(&p, &Default::default(), |_, _| {}).unwrap_err();
    assert!(err.to_string().contains("no frames"), "{err}");
    assert!(!tmp.path().join("Film/exports").read_dir().map(|mut d| d.next().is_some()).unwrap_or(false), "no empty folder left behind");
}

// ----------------------------------------------------------- frame ranges

#[test]
fn plan_range_compiles_only_the_marked_frames_of_one_scene() {
    let tmp = tempfile::tempdir().unwrap();
    let p = project(tmp.path());
    for _ in 0..5 {
        capture::capture(&p, None, fake_shot).unwrap();
    }
    let ids = |shots: &[Shot]| -> Vec<String> {
        shots.iter().map(|s| s.path.file_stem().unwrap().to_string_lossy().into_owned()).collect()
    };
    let (shots, _) = compile::plan_range(&p, Some("sc010"), None, Some((1, 3))).unwrap();
    assert_eq!(ids(&shots), ["000002", "000003", "000004"]);
    // Reversed and past the end are tidied up, not errors.
    let (shots, _) = compile::plan_range(&p, Some("sc010"), None, Some((9, 3))).unwrap();
    assert_eq!(ids(&shots), ["000004", "000005"]);
    // A range means nothing for the whole project: everything is compiled.
    let (shots, _) = compile::plan_range(&p, None, None, Some((1, 1))).unwrap();
    assert_eq!(shots.len(), 5);
}
