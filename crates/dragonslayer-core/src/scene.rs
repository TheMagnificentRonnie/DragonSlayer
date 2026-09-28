use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, IoContext, Result};
use crate::journal::{self, JournalEntry, JournalOp};
use crate::{atomic, paths};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SceneFile {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub fps: Option<u32>,
    /// Reference audio (music, dialogue) the scene is animated to. Takes use their scene's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<SceneAudio>,
}

/// A sound file in the project's `audio/` folder, and where in it frame 1 falls.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SceneAudio {
    /// Path relative to the project folder, e.g. `audio/song.wav`.
    pub file: String,
    /// Milliseconds into the sound at frame 1.
    #[serde(default)]
    pub start_ms: u64,
}

#[derive(Debug, Clone)]
pub struct Frame {
    /// Six-digit number, e.g. `000001`.
    pub id: String,
    pub camera: Option<String>,
    /// All files for this frame in `frames/` (JPEG, RAW, ...).
    pub files: Vec<PathBuf>,
}

impl Frame {
    pub fn jpeg(&self) -> Option<&Path> {
        self.files.iter().map(PathBuf::as_path).find(|p| is_jpeg(p))
    }
}

pub fn is_jpeg(p: &Path) -> bool {
    matches!(
        p.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref(),
        Some("jpg" | "jpeg")
    )
}

#[derive(Debug, Clone)]
pub struct Scene {
    pub dir: PathBuf,
    pub file: SceneFile,
}

impl Scene {
    pub(crate) fn create(dir: PathBuf, file: SceneFile) -> Result<Self> {
        for d in [&dir, &paths::frames_dir(&dir), &paths::incoming_dir(&dir), &paths::scene_trash(&dir)] {
            fs::create_dir_all(d).at(d)?;
        }
        let scene = Self { dir, file };
        scene.save()?;
        Ok(scene)
    }

    pub fn load(dir: PathBuf) -> Result<Self> {
        let path = dir.join(paths::SCENE_FILE);
        let bytes = fs::read(&path).at(&path)?;
        let file = serde_json::from_slice(&bytes).map_err(|source| Error::Json { path, source })?;
        Ok(Self { dir, file })
    }

    pub fn save(&self) -> Result<()> {
        atomic::write_json_atomic(&self.dir.join(paths::SCENE_FILE), &self.file)
    }

    pub fn id(&self) -> &str {
        &self.file.id
    }

    pub fn name(&self) -> &str {
        &self.file.name
    }

    pub fn journal_path(&self) -> PathBuf {
        self.dir.join(paths::JOURNAL_FILE)
    }

    pub fn journal(&self) -> Result<Vec<JournalEntry>> {
        journal::read(&self.journal_path())
    }

    pub(crate) fn append(&self, entry: &JournalEntry) -> Result<()> {
        journal::append(&self.journal_path(), entry)
    }

    /// Frames in capture order, rebuilt from the journal.
    pub fn frames(&self) -> Result<Vec<Frame>> {
        let state = replay(&self.journal()?);
        let mut files = self.files_by_stem(&paths::frames_dir(&self.dir))?;
        Ok(state
            .frames
            .into_iter()
            .map(|(id, camera)| {
                let files = files.remove(&id).unwrap_or_default();
                Frame { id, camera, files }
            })
            .collect())
    }

    pub fn frame_count(&self) -> Result<usize> {
        Ok(replay(&self.journal()?).frames.len())
    }

    /// Frame numbers that were started but never committed or abandoned.
    pub fn pending(&self) -> Result<Vec<String>> {
        Ok(replay(&self.journal()?).pending.into_iter().collect())
    }

    /// Numbers are never reused, so a trashed frame can't collide with a new one.
    pub fn next_frame_id(&self) -> Result<String> {
        let max = self
            .journal()?
            .iter()
            .filter_map(|e| e.frame.parse::<u64>().ok())
            .max()
            .unwrap_or(0);
        Ok(format!("{:06}", max + 1))
    }

    pub(crate) fn files_by_stem(&self, dir: &Path) -> Result<BTreeMap<String, Vec<PathBuf>>> {
        let mut map: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
        let entries = match fs::read_dir(dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(map),
            Err(e) => return Err(Error::io(dir, e)),
        };
        for entry in entries {
            let path = entry.at(dir)?.path();
            if !path.is_file() {
                continue;
            }
            // Key on the part before the first dot so `000001.2.jpg` groups with `000001`.
            if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
                let key = name.split('.').next().unwrap_or(name).to_owned();
                map.entry(key).or_default().push(path);
            }
        }
        for v in map.values_mut() {
            v.sort();
        }
        Ok(map)
    }

    /// Moves the last frame's files to the scene's `trash/`. Returns its id.
    pub fn delete_last(&self) -> Result<String> {
        let frame = self
            .frames()?
            .pop()
            .ok_or_else(|| Error::NoFrames(self.id().to_owned()))?;
        let trash = paths::scene_trash(&self.dir);
        fs::create_dir_all(&trash).at(&trash)?;
        for f in &frame.files {
            let name = f.file_name().expect("file").to_string_lossy();
            let dest = paths::unique_path(&trash, &name);
            fs::rename(f, &dest).at(f)?;
        }
        atomic::sync_dir(&trash);
        atomic::sync_dir(&paths::frames_dir(&self.dir));
        self.append(&JournalEntry::now(JournalOp::Delete, &frame.id, None))?;
        Ok(frame.id)
    }
}

struct Replay {
    frames: Vec<(String, Option<String>)>,
    pending: BTreeSet<String>,
}

fn replay(entries: &[JournalEntry]) -> Replay {
    let mut frames: Vec<(String, Option<String>)> = Vec::new();
    let mut pending = BTreeSet::new();
    for e in entries {
        match e.op {
            JournalOp::Pending => {
                pending.insert(e.frame.clone());
            }
            JournalOp::Capture => {
                pending.remove(&e.frame);
                if !frames.iter().any(|(id, _)| *id == e.frame) {
                    frames.push((e.frame.clone(), e.camera.clone()));
                }
            }
            JournalOp::Abandon => {
                pending.remove(&e.frame);
            }
            JournalOp::Delete => frames.retain(|(id, _)| *id != e.frame),
        }
    }
    Replay { frames, pending }
}
