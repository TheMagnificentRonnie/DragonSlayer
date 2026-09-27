//! Import photos from a folder or memory card into a scene: rebuilding a film from the
//! camera card when the project was lost or damaged, or pulling in shots taken without
//! the app. The source is only ever read. Each frame goes through the same transaction
//! as a live capture, so an interrupted import is recovered like an interrupted shot.

use std::collections::{BTreeMap, HashSet};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::capture;
use crate::error::{Error, Result};
use crate::scene::{is_jpeg, Scene};

const RAW_EXTS: [&str; 15] =
    ["cr2", "cr3", "crw", "nef", "nrw", "arw", "srf", "sr2", "rw2", "raf", "orf", "ori", "pef", "dng", "raw"];

/// Recorded as the "camera" on imported frames.
pub const IMPORTED: &str = "Imported";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Order {
    /// When each photo was taken (the file time the camera wrote). Survives the camera's
    /// file numbering rolling over from 9999 to 0001.
    #[default]
    Taken,
    /// By folder and file name.
    Name,
}

/// One frame to import: every file of one shot (e.g. `IMG_0421.JPG` + `IMG_0421.CR2`).
#[derive(Debug, Clone)]
pub struct Candidate {
    /// Folder and name without extension, e.g. `100CANON/IMG_0421`. Stored in the journal.
    pub source: String,
    pub files: Vec<PathBuf>,
    pub taken: SystemTime,
}

impl Candidate {
    pub fn has_jpeg(&self) -> bool {
        self.files.iter().any(|f| is_jpeg(f))
    }
}

/// What an import would do, worked out before anything is copied.
#[derive(Debug, Default)]
pub struct Plan {
    pub frames: Vec<Candidate>,
    /// Shots skipped because this scene already has them from an earlier import.
    pub already_imported: usize,
    /// Files that aren't photos (videos, thumbnails, sidecars).
    pub ignored: usize,
    /// Folders or files that couldn't be read at all (common on a damaged card).
    pub unreadable: Vec<(PathBuf, String)>,
}

impl Plan {
    pub fn raw_only(&self) -> usize {
        self.frames.iter().filter(|c| !c.has_jpeg()).count()
    }
}

#[derive(Debug, Default)]
pub struct Report {
    /// (frame id, source)
    pub imported: Vec<(String, String)>,
    /// (source, why) for shots that couldn't be copied. Nothing of them was kept.
    pub failed: Vec<(String, String)>,
    /// Imported, but the JPEG looks cut short: check these frames.
    pub truncated: Vec<(String, String)>,
    /// The user stopped the import before the end.
    pub stopped: bool,
    /// A problem that ended the import early, e.g. the project disk filled up.
    pub aborted: Option<String>,
}

pub fn is_photo(p: &Path) -> bool {
    is_jpeg(p)
        || p.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| RAW_EXTS.contains(&e.to_ascii_lowercase().as_str()))
}

/// Finds the shots in `paths` (files, or folders searched recursively), groups each
/// shot's files, orders them, and leaves out shots `scene` already imported.
pub fn scan(paths: &[PathBuf], order: Order, scene: Option<&Scene>) -> Result<Plan> {
    let mut plan = Plan::default();
    let mut groups: BTreeMap<String, Candidate> = BTreeMap::new();
    for p in paths {
        walk(p, &mut groups, &mut plan);
    }

    let done: HashSet<String> = match scene {
        Some(s) => s.journal()?.into_iter().filter_map(|e| e.source).map(|s| s.to_lowercase()).collect(),
        None => HashSet::new(),
    };
    let mut frames: Vec<Candidate> = Vec::new();
    for c in groups.into_values() {
        if done.contains(&c.source.to_lowercase()) {
            plan.already_imported += 1;
        } else {
            frames.push(c);
        }
    }
    match order {
        Order::Taken => frames.sort_by(|a, b| a.taken.cmp(&b.taken).then_with(|| a.source.cmp(&b.source))),
        Order::Name => frames.sort_by_key(|c| c.source.to_lowercase()),
    }
    plan.frames = frames;
    Ok(plan)
}

fn walk(path: &Path, groups: &mut BTreeMap<String, Candidate>, plan: &mut Plan) {
    let meta = match fs::metadata(path) {
        Ok(m) => m,
        Err(e) => return plan.unreadable.push((path.to_owned(), e.to_string())),
    };
    if meta.is_dir() {
        let entries = match fs::read_dir(path) {
            Ok(e) => e,
            Err(e) => return plan.unreadable.push((path.to_owned(), e.to_string())),
        };
        let mut children: Vec<PathBuf> = Vec::new();
        for entry in entries {
            match entry {
                Ok(e) => children.push(e.path()),
                Err(e) => plan.unreadable.push((path.to_owned(), e.to_string())),
            }
        }
        children.sort();
        for child in children {
            // Skip hidden and system folders (.Trashes, .Spotlight-V100, System Volume Information).
            let name = child.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if name.starts_with('.') || name.eq_ignore_ascii_case("System Volume Information") {
                continue;
            }
            walk(&child, groups, plan);
        }
        return;
    }
    if !is_photo(path) {
        plan.ignored += 1;
        return;
    }
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let folder = path.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned());
    let source = match folder {
        Some(f) if !f.is_empty() => format!("{f}/{stem}"),
        _ => stem,
    };
    let taken = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    // Group by the real folder, not just its name: two cards scanned together can both
    // have 100CANON/IMG_0001, and those are different photos.
    let dir = path.parent().map(|p| p.to_string_lossy().to_lowercase()).unwrap_or_default();
    let key = format!("{dir}/{}", stem_lower(path));
    let c = groups.entry(key).or_insert_with(|| Candidate { source, files: Vec::new(), taken });
    c.taken = c.taken.min(taken);
    c.files.push(path.to_owned());
}

/// Imports `plan` into `scene`, appending after its existing frames. `on_progress(done, total)`
/// is called before each shot; return `false` from it to stop.
pub fn import(scene: &Scene, plan: &Plan, mut on_progress: impl FnMut(usize, usize) -> bool) -> Report {
    let mut report = Report::default();
    let total = plan.frames.len();
    for (i, c) in plan.frames.iter().enumerate() {
        if !on_progress(i, total) {
            report.stopped = true;
            break;
        }
        let result = capture::capture_into(scene, Some(IMPORTED), Some(&c.source), |dir| copy_shot(&c.files, dir));
        match result {
            Ok(captured) => {
                if captured.files.iter().any(|f| is_jpeg(f) && looks_truncated(f)) {
                    report.truncated.push((captured.frame.clone(), c.source.clone()));
                }
                report.imported.push((captured.frame, c.source.clone()));
            }
            Err(Error::Capture(why)) => report.failed.push((c.source.clone(), why)),
            Err(e) => {
                report.aborted = Some(e.to_string());
                break;
            }
        }
    }
    on_progress(report.imported.len() + report.failed.len(), total);
    report
}

/// Copies one shot's files into the capture's incoming folder. On any read error the
/// partial copies are removed, so the capture is abandoned rather than recovered half-done.
fn copy_shot(files: &[PathBuf], dir: &Path) -> std::result::Result<Vec<PathBuf>, String> {
    let mut out = Vec::new();
    for src in files {
        let name = src.file_name().ok_or("file has no name")?;
        let dest = dir.join(name);
        if let Err(e) = fs::copy(src, &dest) {
            for f in out.iter().chain(std::iter::once(&dest)) {
                let _ = fs::remove_file(f);
            }
            return Err(format!("couldn't read {}: {e}", src.display()));
        }
        out.push(dest);
    }
    Ok(out)
}

/// A JPEG starts with FFD8 and ends with FFD9. A file cut short by a damaged card lacks
/// the end marker. Some cameras pad after it, so look in the last 64 KB, not just the
/// last two bytes; entropy-coded data can't contain FFD9, so a hit there is the real end.
pub fn looks_truncated(path: &Path) -> bool {
    let check = || -> std::io::Result<bool> {
        let mut f = File::open(path)?;
        let len = f.metadata()?.len();
        let mut head = [0u8; 2];
        f.read_exact(&mut head)?;
        if head != [0xFF, 0xD8] {
            return Ok(true);
        }
        let tail_len = len.min(64 * 1024);
        f.seek(SeekFrom::Start(len - tail_len))?;
        let mut tail = vec![0u8; tail_len as usize];
        f.read_exact(&mut tail)?;
        Ok(!tail.windows(2).any(|w| w == [0xFF, 0xD9]))
    };
    check().unwrap_or(true)
}

fn stem_lower(path: &Path) -> String {
    path.file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default()
}
