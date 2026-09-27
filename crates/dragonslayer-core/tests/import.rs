//! Importing photos from a camera card into a scene.

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use dragonslayer_core::import::{self, Order};
use dragonslayer_core::Project;

const JPEG: &[u8] = &[0xFF, 0xD8, 0xFF, 0xE0, 1, 2, 3, 4, 0xFF, 0xD9];
/// Starts like a JPEG but the card lost the end.
const CUT_JPEG: &[u8] = &[0xFF, 0xD8, 0xFF, 0xE0, 1, 2, 3, 4];

fn write(path: &Path, bytes: &[u8], minutes: u64) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
    let t = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000 + minutes * 60);
    File::options().write(true).open(path).unwrap().set_modified(t).unwrap();
}

/// A card as a camera leaves it. Shot order by time: 0421, 0422, 0423, 0424. File names
/// are deliberately out of time order for 0423/0424 (as after the camera's numbering was reset).
fn card(root: &Path) -> PathBuf {
    let dcim = root.join("DCIM");
    write(&dcim.join("100CANON/IMG_0421.JPG"), JPEG, 1);
    write(&dcim.join("100CANON/IMG_0421.CR2"), b"raw", 1);
    write(&dcim.join("100CANON/IMG_0422.JPG"), CUT_JPEG, 2);
    write(&dcim.join("100CANON/IMG_0002.JPG"), JPEG, 4);
    write(&dcim.join("100CANON/IMG_0001.CR2"), b"raw only", 3);
    write(&dcim.join("100CANON/MVI_0500.MOV"), b"video", 5);
    write(&dcim.join(".Trashes/IMG_9999.JPG"), JPEG, 0);
    dcim
}

fn project(dir: &Path) -> Project {
    Project::create(dir.join("Film"), "Film", 12).unwrap()
}

#[test]
fn scan_groups_each_shots_files_orders_by_time_and_skips_the_rest() {
    let tmp = tempfile::tempdir().unwrap();
    let dcim = card(tmp.path());
    let plan = import::scan(&[dcim], Order::Taken, None).unwrap();
    let sources: Vec<_> = plan.frames.iter().map(|c| c.source.as_str()).collect();
    assert_eq!(sources, ["100CANON/IMG_0421", "100CANON/IMG_0422", "100CANON/IMG_0001", "100CANON/IMG_0002"]);
    assert_eq!(plan.frames[0].files.len(), 2, "JPEG + RAW are one frame");
    assert_eq!(plan.ignored, 1, "the video");
    assert_eq!(plan.raw_only(), 1);
    assert!(plan.unreadable.is_empty());
}

#[test]
fn name_order_sorts_by_file_name_instead() {
    let tmp = tempfile::tempdir().unwrap();
    let dcim = card(tmp.path());
    let plan = import::scan(&[dcim], Order::Name, None).unwrap();
    let sources: Vec<_> = plan.frames.iter().map(|c| c.source.as_str()).collect();
    assert_eq!(sources, ["100CANON/IMG_0001", "100CANON/IMG_0002", "100CANON/IMG_0421", "100CANON/IMG_0422"]);
}

#[test]
fn import_appends_frames_flags_cut_jpegs_and_never_touches_the_card() {
    let tmp = tempfile::tempdir().unwrap();
    let dcim = card(tmp.path());
    let p = project(tmp.path());
    let scene = p.active_scene().unwrap();

    let plan = import::scan(std::slice::from_ref(&dcim), Order::Taken, Some(&scene)).unwrap();
    let report = import::import(&scene, &plan, |_, _| true);

    assert_eq!(report.imported.len(), 4);
    assert!(report.failed.is_empty() && report.aborted.is_none() && !report.stopped);
    assert_eq!(report.truncated, [("000002".to_string(), "100CANON/IMG_0422".to_string())]);

    let frames = scene.frames().unwrap();
    assert_eq!(frames.len(), 4);
    assert!(scene.dir.join("frames/000001.jpg").is_file());
    assert!(scene.dir.join("frames/000001.CR2").is_file());
    assert!(frames[2].jpeg().is_none(), "RAW-only shot is kept even though it can't be shown");
    assert!(scene.pending().unwrap().is_empty());

    // The card is only read.
    assert_eq!(fs::read(dcim.join("100CANON/IMG_0421.JPG")).unwrap(), JPEG);
    assert!(dcim.join("100CANON/IMG_0421.CR2").is_file());
}

#[test]
fn rerunning_an_import_skips_shots_already_imported() {
    let tmp = tempfile::tempdir().unwrap();
    let dcim = card(tmp.path());
    let p = project(tmp.path());
    let scene = p.active_scene().unwrap();
    let plan = import::scan(std::slice::from_ref(&dcim), Order::Taken, Some(&scene)).unwrap();
    import::import(&scene, &plan, |_, _| true);

    // A photo recovered from the card afterwards: only that one is new.
    write(&dcim.join("100CANON/IMG_0423.JPG"), JPEG, 6);
    let again = import::scan(&[dcim], Order::Taken, Some(&scene)).unwrap();
    assert_eq!(again.already_imported, 4);
    let sources: Vec<_> = again.frames.iter().map(|c| c.source.as_str()).collect();
    assert_eq!(sources, ["100CANON/IMG_0423"]);
}

#[test]
fn a_shot_that_cant_be_read_is_skipped_cleanly_and_the_rest_import() {
    let tmp = tempfile::tempdir().unwrap();
    let dcim = card(tmp.path());
    let p = project(tmp.path());
    let scene = p.active_scene().unwrap();
    let plan = import::scan(std::slice::from_ref(&dcim), Order::Taken, Some(&scene)).unwrap();
    // The RAW of the first shot becomes unreadable after the scan (a flaky card).
    fs::remove_file(dcim.join("100CANON/IMG_0421.CR2")).unwrap();

    let report = import::import(&scene, &plan, |_, _| true);
    assert_eq!(report.failed.len(), 1);
    assert_eq!(report.failed[0].0, "100CANON/IMG_0421");
    assert_eq!(report.imported.len(), 3);
    // Nothing half-copied is left behind or recovered later.
    assert!(scene.pending().unwrap().is_empty());
    assert!(fs::read_dir(scene.dir.join("incoming")).unwrap().next().is_none());
    let report = p.recover().unwrap();
    assert!(report.recovered.is_empty());
    assert_eq!(scene.frame_count().unwrap(), 3);
}

#[test]
fn import_can_be_stopped_part_way() {
    let tmp = tempfile::tempdir().unwrap();
    let dcim = card(tmp.path());
    let p = project(tmp.path());
    let scene = p.active_scene().unwrap();
    let plan = import::scan(&[dcim], Order::Taken, Some(&scene)).unwrap();
    let report = import::import(&scene, &plan, |done, _| done < 2);
    assert!(report.stopped);
    assert_eq!(report.imported.len(), 2);
    assert_eq!(scene.frame_count().unwrap(), 2);
}

#[test]
fn imported_frames_follow_existing_ones_in_the_scene() {
    let tmp = tempfile::tempdir().unwrap();
    let dcim = card(tmp.path());
    let p = project(tmp.path());
    let scene = p.active_scene().unwrap();
    dragonslayer_core::capture::capture(&p, None, |dir| {
        let f = dir.join("live.jpg");
        fs::write(&f, JPEG).unwrap();
        Ok(vec![f])
    })
    .unwrap();
    let plan = import::scan(&[dcim], Order::Taken, Some(&scene)).unwrap();
    let report = import::import(&scene, &plan, |_, _| true);
    assert_eq!(report.imported[0].0, "000002");
    assert_eq!(scene.frame_count().unwrap(), 5);
}

#[test]
fn truncation_check() {
    let tmp = tempfile::tempdir().unwrap();
    let good = tmp.path().join("good.jpg");
    let cut = tmp.path().join("cut.jpg");
    let padded = tmp.path().join("padded.jpg");
    let not_jpeg = tmp.path().join("fake.jpg");
    fs::write(&good, JPEG).unwrap();
    fs::write(&cut, CUT_JPEG).unwrap();
    fs::write(&padded, [JPEG, &[0u8; 300]].concat()).unwrap();
    fs::write(&not_jpeg, b"hello").unwrap();
    assert!(!import::looks_truncated(&good));
    assert!(import::looks_truncated(&cut));
    assert!(!import::looks_truncated(&padded), "padding after the end marker is fine");
    assert!(import::looks_truncated(&not_jpeg));
    assert!(import::looks_truncated(&tmp.path().join("missing.jpg")));
}
