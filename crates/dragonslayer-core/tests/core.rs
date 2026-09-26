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

    let (shots, warnings) = compile::plan(&p, None).unwrap();
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
    assert!(compile::plan(&p, Some("Empty")).is_err());
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
