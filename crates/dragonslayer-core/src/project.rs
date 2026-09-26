use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, IoContext, Result};
use crate::journal::{JournalEntry, JournalOp};
use crate::scene::{Scene, SceneFile};
use crate::{atomic, capture, paths};

pub const FORMAT: &str = "dragonslayer/1";
pub const DEFAULT_FPS: u32 = 12;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectFile {
    pub format: String,
    pub name: String,
    pub fps: u32,
    pub scenes: Vec<String>,
    pub active_scene: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Project {
    pub root: PathBuf,
    pub file: ProjectFile,
}

#[derive(Debug, Default)]
pub struct RecoveryReport {
    /// (scene id, frame id) committed from `incoming/`.
    pub recovered: Vec<(String, String)>,
    /// (scene id, frame id) with no files to recover; reshoot from the camera card if needed.
    pub abandoned: Vec<(String, String)>,
}

impl Project {
    /// Creates the folder layout with one empty, active scene.
    pub fn create(root: impl Into<PathBuf>, name: &str, fps: u32) -> Result<Self> {
        let root = root.into();
        if root.join(paths::PROJECT_FILE).exists() {
            return Err(Error::ProjectExists(root));
        }
        for d in [
            root.clone(),
            paths::scenes_dir(&root),
            paths::project_trash(&root),
            paths::exports_dir(&root),
        ] {
            fs::create_dir_all(&d).at(&d)?;
        }
        let mut project = Self {
            root,
            file: ProjectFile {
                format: FORMAT.into(),
                name: name.into(),
                fps,
                scenes: Vec::new(),
                active_scene: None,
            },
        };
        project.add_scene("Scene 1", None)?;
        Ok(project)
    }

    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        let path = root.join(paths::PROJECT_FILE);
        let bytes = fs::read(&path).at(&path)?;
        let file: ProjectFile =
            serde_json::from_slice(&bytes).map_err(|source| Error::Json { path, source })?;
        if file.format != FORMAT {
            return Err(Error::UnsupportedFormat { found: file.format, expected: FORMAT });
        }
        Ok(Self { root, file })
    }

    pub fn save(&self) -> Result<()> {
        atomic::write_json_atomic(&self.root.join(paths::PROJECT_FILE), &self.file)
    }

    pub fn name(&self) -> &str {
        &self.file.name
    }

    pub fn scene(&self, id: &str) -> Result<Scene> {
        if !self.file.scenes.iter().any(|s| s == id) {
            return Err(Error::SceneNotFound(id.into()));
        }
        Scene::load(paths::scene_dir(&self.root, id))
    }

    /// Scenes in project order.
    pub fn scenes(&self) -> Result<Vec<Scene>> {
        self.file.scenes.iter().map(|id| self.scene(id)).collect()
    }

    pub fn active_scene(&self) -> Result<Scene> {
        let id = self.file.active_scene.as_deref().ok_or(Error::NoActiveScene)?;
        self.scene(id)
    }

    /// Resolves a scene by id, or by display name if no id matches.
    pub fn find_scene(&self, key: &str) -> Result<Scene> {
        if self.file.scenes.iter().any(|s| s == key) {
            return self.scene(key);
        }
        self.scenes()?
            .into_iter()
            .find(|s| s.name() == key)
            .ok_or_else(|| Error::SceneNotFound(key.into()))
    }

    pub fn fps_for(&self, scene: &Scene) -> u32 {
        scene.file.fps.unwrap_or(self.file.fps)
    }

    /// Adds a scene after `after` (or at the end) and makes it active.
    pub fn add_scene(&mut self, name: &str, after: Option<&str>) -> Result<Scene> {
        let pos = match after {
            Some(a) => {
                self.index_of(a)? + 1
            }
            None => self.file.scenes.len(),
        };
        let id = self.new_scene_id(pos);
        let scene = Scene::create(
            paths::scene_dir(&self.root, &id),
            SceneFile { id: id.clone(), name: name.into(), fps: None },
        )?;
        self.file.scenes.insert(pos, id.clone());
        self.file.active_scene = Some(id);
        self.save()?;
        Ok(scene)
    }

    pub fn rename_scene(&mut self, id: &str, name: &str) -> Result<()> {
        let mut scene = self.scene(id)?;
        scene.file.name = name.into();
        scene.save()
    }

    pub fn set_scene_fps(&mut self, id: &str, fps: Option<u32>) -> Result<()> {
        let mut scene = self.scene(id)?;
        scene.file.fps = fps;
        scene.save()
    }

    /// Moves a scene to `to` (0-based, clamped).
    pub fn move_scene(&mut self, id: &str, to: usize) -> Result<()> {
        let from = self.index_of(id)?;
        let id = self.file.scenes.remove(from);
        let to = to.min(self.file.scenes.len());
        self.file.scenes.insert(to, id);
        self.save()
    }

    /// Moves the scene folder into the project `trash/`.
    pub fn delete_scene(&mut self, id: &str) -> Result<()> {
        let idx = self.index_of(id)?;
        let src = paths::scene_dir(&self.root, id);
        let trash = paths::project_trash(&self.root);
        fs::create_dir_all(&trash).at(&trash)?;
        let dest = paths::unique_path(&trash, id);
        fs::rename(&src, &dest).at(&src)?;
        self.file.scenes.remove(idx);
        if self.file.active_scene.as_deref() == Some(id) {
            let next = idx.min(self.file.scenes.len().saturating_sub(1));
            self.file.active_scene = self.file.scenes.get(next).cloned();
        }
        self.save()
    }

    pub fn set_active(&mut self, id: &str) -> Result<()> {
        self.index_of(id)?;
        self.file.active_scene = Some(id.into());
        self.save()
    }

    /// Commits or abandons captures left pending by a crash. Run on launch.
    pub fn recover(&self) -> Result<RecoveryReport> {
        let mut report = RecoveryReport::default();
        for scene in self.scenes()? {
            let pending = scene.pending()?;
            if pending.is_empty() {
                continue;
            }
            for frame in pending {
                let files = list_files(&capture::pending_dir(&scene, &frame))?;
                match files {
                    files if !files.is_empty() => {
                        capture::commit(&scene, &frame, &files, None)?;
                        report.recovered.push((scene.id().into(), frame));
                    }
                    _ => {
                        scene.append(&JournalEntry::now(JournalOp::Abandon, &frame, None))?;
                        report.abandoned.push((scene.id().into(), frame));
                    }
                }
            }
        }
        Ok(report)
    }

    fn index_of(&self, id: &str) -> Result<usize> {
        self.file
            .scenes
            .iter()
            .position(|s| s == id)
            .ok_or_else(|| Error::SceneNotFound(id.into()))
    }

    /// `sc010`, `sc020`, ... at the end; the midpoint (`sc015`) when inserting between two.
    fn new_scene_id(&self, pos: usize) -> String {
        let used: BTreeSet<u32> = self.file.scenes.iter().filter_map(|s| parse_scene_id(s)).collect();
        // Trashed scene folders keep their names, so avoid those too.
        let trash = paths::project_trash(&self.root);
        let taken = |n: u32| used.contains(&n) || trash.join(format_scene_id(n)).exists();
        let max = used.iter().copied().max().unwrap_or(0);

        let prev = pos.checked_sub(1).and_then(|i| self.file.scenes.get(i)).and_then(|s| parse_scene_id(s));
        let next = self.file.scenes.get(pos).and_then(|s| parse_scene_id(s));
        if let (Some(a), Some(b)) = (prev, next)
            && b > a + 1 {
                let mid = a + (b - a) / 2;
                if !taken(mid) {
                    return format_scene_id(mid);
                }
            }
        let mut n = (max / 10 + 1) * 10;
        while taken(n) {
            n += 10;
        }
        format_scene_id(n)
    }
}

fn list_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(Error::io(dir, e)),
    };
    let mut out = Vec::new();
    for entry in entries {
        let p = entry.at(dir)?.path();
        if p.is_file() {
            out.push(p);
        }
    }
    out.sort();
    Ok(out)
}

fn parse_scene_id(id: &str) -> Option<u32> {
    id.strip_prefix("sc")?.parse().ok()
}

fn format_scene_id(n: u32) -> String {
    format!("sc{n:03}")
}

pub fn is_project(path: &Path) -> bool {
    path.join(paths::PROJECT_FILE).is_file()
}
