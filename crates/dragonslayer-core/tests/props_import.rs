//! Property tests: import from random camera-card layouts.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use dragonslayer_core::import::{self, Order};
use dragonslayer_core::Project;
use proptest::prelude::*;

const FOLDERS: [&str; 4] = ["100CANON", "101CANON", "100_PANA", ".Trashes"];
/// Each stem has one fixed spelling, so no two differ only by case (Windows and macOS
/// filesystems are case-insensitive; a case-only twin would overwrite the other file).
const STEMS: [&str; 7] = ["IMG_0001", "img_0002", "DSC_0003", "P1000004", "_MG_0005", "IMG_9999", "IMG_0421"];
/// (spelling, is photo, is JPEG). JPG/JPEG are distinct files; case variants of one
/// extension are chosen per file below.
const EXTS: [(&str, bool, bool); 9] = [
    ("JPG", true, true),
    ("JPEG", true, true),
    ("CR2", true, false),
    ("NEF", true, false),
    ("RW2", true, false),
    ("MOV", false, false),
    ("THM", false, false),
    ("XMP", false, false),
    ("txt", false, false),
];

const JPEG: &[u8] = &[0xFF, 0xD8, 1, 2, 3, 0xFF, 0xD9];
const CUT: &[u8] = &[0xFF, 0xD8, 1, 2, 3];

#[derive(Debug, Clone)]
struct CardFile {
    folder: usize,
    stem: usize,
    ext: usize,
    lower_ext: bool,
    minutes: u64,
    cut: bool,
}

fn card_file() -> impl Strategy<Value = CardFile> {
    (0..FOLDERS.len(), 0..STEMS.len(), 0..EXTS.len(), any::<bool>(), 0u64..500, any::<bool>())
        .prop_map(|(folder, stem, ext, lower_ext, minutes, cut)| CardFile { folder, stem, ext, lower_ext, minutes, cut })
}

struct Written {
    path: PathBuf,
    folder: usize,
    stem: usize,
    photo: bool,
    jpeg: bool,
    cut: bool,
    minutes: u64,
}

/// Writes the card; later duplicates of the same (folder, stem, ext) replace earlier ones.
fn write_card(root: &Path, files: &[CardFile]) -> Vec<Written> {
    let mut by_key: BTreeMap<(usize, usize, usize), &CardFile> = BTreeMap::new();
    for f in files {
        by_key.insert((f.folder, f.stem, f.ext), f);
    }
    fs::create_dir_all(root.join("DCIM")).unwrap();
    let mut out = Vec::new();
    for f in by_key.values() {
        let (ext, photo, jpeg) = EXTS[f.ext];
        let ext = if f.lower_ext { ext.to_lowercase() } else { ext.to_owned() };
        let path = root.join("DCIM").join(FOLDERS[f.folder]).join(format!("{}.{ext}", STEMS[f.stem]));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let bytes: &[u8] = if jpeg { if f.cut { CUT } else { JPEG } } else { b"other" };
        fs::write(&path, bytes).unwrap();
        let t = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000 + f.minutes * 60);
        File::options().write(true).open(&path).unwrap().set_modified(t).unwrap();
        out.push(Written { path, folder: f.folder, stem: f.stem, photo, jpeg, cut: jpeg && f.cut, minutes: f.minutes });
    }
    out
}

fn hidden(w: &Written) -> bool {
    FOLDERS[w.folder].starts_with('.')
}

fn snapshot(files: &[Written]) -> Vec<(PathBuf, Vec<u8>, SystemTime)> {
    files
        .iter()
        .map(|w| (w.path.clone(), fs::read(&w.path).unwrap(), fs::metadata(&w.path).unwrap().modified().unwrap()))
        .collect()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn scan_groups_every_photo_exactly_once_and_orders_correctly(
        files in proptest::collection::vec(card_file(), 0..30),
        by_name in any::<bool>(),
    ) {
        let tmp = tempfile::tempdir().unwrap();
        let written = write_card(tmp.path(), &files);
        let order = if by_name { Order::Name } else { Order::Taken };
        let plan = import::scan(&[tmp.path().join("DCIM")], order, None).unwrap();

        let visible: Vec<&Written> = written.iter().filter(|w| !hidden(w)).collect();
        let photos: BTreeSet<PathBuf> = visible.iter().filter(|w| w.photo).map(|w| w.path.clone()).collect();
        let shots: BTreeSet<(usize, usize)> = visible.iter().filter(|w| w.photo).map(|w| (w.folder, w.stem)).collect();

        prop_assert_eq!(plan.frames.len(), shots.len());
        prop_assert_eq!(plan.ignored, visible.iter().filter(|w| !w.photo).count());
        prop_assert!(plan.unreadable.is_empty());
        prop_assert_eq!(plan.already_imported, 0);
        // Every photo is in exactly one frame; no hidden or non-photo file is.
        let mut seen = Vec::new();
        for c in &plan.frames {
            prop_assert!(!c.files.is_empty());
            let stems: BTreeSet<String> = c.files.iter().map(|f| f.file_stem().unwrap().to_string_lossy().into_owned()).collect();
            prop_assert_eq!(stems.len(), 1, "one shot's files share a stem: {:?}", c.files);
            seen.extend(c.files.iter().cloned());
        }
        prop_assert_eq!(seen.len(), photos.len(), "a photo was grouped twice or dropped");
        prop_assert_eq!(seen.into_iter().collect::<BTreeSet<_>>(), photos);
        // RAW-only count matches the card.
        let raw_only = shots.iter().filter(|s| !visible.iter().any(|w| (w.folder, w.stem) == **s && w.jpeg)).count();
        prop_assert_eq!(plan.raw_only(), raw_only);
        // Ordering.
        for pair in plan.frames.windows(2) {
            match order {
                Order::Taken => prop_assert!(pair[0].taken <= pair[1].taken, "not in shooting order"),
                Order::Name => prop_assert!(pair[0].source.to_lowercase() <= pair[1].source.to_lowercase()),
            }
        }
        if order == Order::Taken {
            for c in &plan.frames {
                let min = visible.iter().filter(|w| w.photo && c.files.contains(&w.path)).map(|w| w.minutes).min().unwrap();
                let expected = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000 + min * 60);
                prop_assert_eq!(c.taken, expected, "a shot is dated by its earliest file");
            }
        }
    }

    #[test]
    fn import_is_complete_read_only_and_idempotent(files in proptest::collection::vec(card_file(), 1..16)) {
        let tmp = tempfile::tempdir().unwrap();
        let written = write_card(&tmp.path().join("card"), &files);
        let before = snapshot(&written);
        let dcim = tmp.path().join("card").join("DCIM");
        let p = Project::create(tmp.path().join("Film"), "Film", 12).unwrap();
        let scene = p.active_scene().unwrap();

        let plan = import::scan(std::slice::from_ref(&dcim), Order::Taken, Some(&scene)).unwrap();
        let report = import::import(&scene, &plan, |_, _| true);
        prop_assert!(report.failed.is_empty() && report.aborted.is_none() && !report.stopped);
        prop_assert_eq!(report.imported.len(), plan.frames.len());
        prop_assert_eq!(scene.frame_count().unwrap(), plan.frames.len());
        // Frames come out in the plan's order, with its sources.
        let sources: Vec<&str> = report.imported.iter().map(|(_, s)| s.as_str()).collect();
        let planned: Vec<&str> = plan.frames.iter().map(|c| c.source.as_str()).collect();
        prop_assert_eq!(sources, planned);
        // Every file of every shot landed in frames/.
        let files_in: usize = scene.frames().unwrap().iter().map(|f| f.files.len()).sum();
        prop_assert_eq!(files_in, plan.frames.iter().map(|c| c.files.len()).sum::<usize>());
        // Cut JPEGs are flagged, and only those.
        let cut_sources: BTreeSet<String> = plan
            .frames
            .iter()
            .filter(|c| c.files.iter().any(|f| written.iter().any(|w| w.path == *f && w.cut)))
            .map(|c| c.source.clone())
            .collect();
        let flagged: BTreeSet<String> = report.truncated.iter().map(|(_, s)| s.clone()).collect();
        prop_assert_eq!(flagged, cut_sources);
        prop_assert!(scene.pending().unwrap().is_empty());

        // The card is untouched: same files, same bytes, same times.
        prop_assert_eq!(snapshot(&written), before);

        // Running it again adds nothing.
        let again = import::scan(&[dcim], Order::Taken, Some(&scene)).unwrap();
        prop_assert_eq!(again.already_imported, plan.frames.len());
        prop_assert!(again.frames.is_empty());
    }

    #[test]
    fn looks_truncated_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..200_000)) {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("x.jpg");
        fs::write(&f, &bytes).unwrap();
        let cut = import::looks_truncated(&f);
        if bytes.len() < 2 || bytes[..2] != [0xFF, 0xD8] {
            prop_assert!(cut);
        }
    }

    #[test]
    fn is_photo_never_panics_and_knows_jpegs(name in "\\PC{0,20}") {
        let p = Path::new(&name);
        let _ = import::is_photo(p);
        prop_assert!(import::is_photo(&Path::new(&name).with_extension("JpG")) || name.is_empty() || p.file_name().is_none());
    }
}

/// Regression: two cards (or two copies of a card) scanned together both have
/// 100CANON/IMG_0001. Grouping by folder name + stem merged the two different photos
/// into one frame. Shots are grouped by their real folder now.
#[test]
fn same_names_on_two_cards_are_separate_shots() {
    let tmp = tempfile::tempdir().unwrap();
    let a = tmp.path().join("cardA/DCIM/100CANON/IMG_0001.JPG");
    let b = tmp.path().join("cardB/DCIM/100CANON/IMG_0001.JPG");
    for (p, bytes) in [(&a, JPEG), (&b, CUT)] {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, bytes).unwrap();
    }
    let plan = import::scan(&[tmp.path().join("cardA"), tmp.path().join("cardB")], Order::Name, None).unwrap();
    assert_eq!(plan.frames.len(), 2, "{:?}", plan.frames);
    assert!(plan.frames.iter().all(|c| c.files.len() == 1));
}
