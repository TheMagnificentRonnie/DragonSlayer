//! Capture as a transaction (spec §9): pre-flight, trigger, download, commit.

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{Error, IoContext, Result};
use crate::journal::{JournalEntry, JournalOp};
use crate::scene::Scene;
use crate::{atomic, paths, Project};

/// Refuse to capture below this much free space (a RAW+JPEG pair can be ~60 MB).
pub const MIN_FREE_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug)]
pub struct Captured {
    pub scene: String,
    pub frame: String,
    pub files: Vec<PathBuf>,
}

/// Runs one capture into the active scene.
///
/// `shoot` triggers the camera and downloads every resulting file into the
/// directory it is given, returning their paths. The frame is only reported
/// back once its files and journal entry are flushed to disk.
pub fn capture<F>(project: &Project, camera: Option<&str>, shoot: F) -> Result<Captured>
where
    F: FnOnce(&Path) -> std::result::Result<Vec<PathBuf>, String>,
{
    capture_into(&project.active_scene()?, camera, None, shoot)
}

/// [`capture`] into a given scene. `source` records where the files came from (imports).
pub fn capture_into<F>(scene: &Scene, camera: Option<&str>, source: Option<&str>, shoot: F) -> Result<Captured>
where
    F: FnOnce(&Path) -> std::result::Result<Vec<PathBuf>, String>,
{
    preflight(scene)?;

    let frame = scene.next_frame_id()?;
    let source = source.map(str::to_owned);
    scene.append(&JournalEntry::now(JournalOp::Pending, &frame, camera.map(Into::into)).with_source(source.clone()))?;

    let dir = pending_dir(scene, &frame);
    fs::create_dir_all(&dir).at(&dir)?;

    let files = match shoot(&dir) {
        Ok(files) if !files.is_empty() => files,
        result => {
            let err = match result {
                Err(e) => Error::Capture(e),
                Ok(_) => Error::NothingDownloaded,
            };
            // If nothing reached the disk there's nothing to recover; otherwise leave it pending.
            if dir_is_empty(&dir) {
                let _ = fs::remove_dir(&dir);
                scene.append(&JournalEntry::now(JournalOp::Abandon, &frame, None))?;
            }
            return Err(err);
        }
    };

    let files = commit(scene, &frame, &files, camera, source)?;
    Ok(Captured { scene: scene.id().into(), frame, files })
}

pub(crate) fn pending_dir(scene: &Scene, frame: &str) -> PathBuf {
    paths::incoming_dir(&scene.dir).join(frame)
}

fn preflight(scene: &Scene) -> Result<()> {
    let available = fs4::available_space(&scene.dir).at(&scene.dir)?;
    if available < MIN_FREE_BYTES {
        return Err(Error::DiskFull { available, required: MIN_FREE_BYTES });
    }
    Ok(())
}

/// Flush each file, move it into `frames/` as `<frame>.<ext>`, then journal the capture.
pub(crate) fn commit(
    scene: &Scene,
    frame: &str,
    files: &[PathBuf],
    camera: Option<&str>,
    source: Option<String>,
) -> Result<Vec<PathBuf>> {
    let frames = paths::frames_dir(&scene.dir);
    fs::create_dir_all(&frames).at(&frames)?;

    let mut out = Vec::with_capacity(files.len());
    for src in files {
        atomic::fsync_file(src)?;
        let ext = normalised_ext(src);
        let mut dest = frames.join(format!("{frame}.{ext}"));
        let mut n = 2;
        while dest.exists() {
            dest = frames.join(format!("{frame}.{n}.{ext}"));
            n += 1;
        }
        fs::rename(src, &dest).at(src)?;
        out.push(dest);
    }
    atomic::sync_dir(&frames);
    let _ = fs::remove_dir(pending_dir(scene, frame));

    scene.append(&JournalEntry::now(JournalOp::Capture, frame, camera.map(Into::into)).with_source(source))?;
    Ok(out)
}

fn normalised_ext(p: &Path) -> String {
    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("bin");
    match ext.to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => "jpg".into(),
        _ => ext.to_ascii_uppercase(),
    }
}

fn dir_is_empty(dir: &Path) -> bool {
    fs::read_dir(dir).map(|mut d| d.next().is_none()).unwrap_or(true)
}
