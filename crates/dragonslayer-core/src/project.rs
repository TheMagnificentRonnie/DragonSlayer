use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, IoContext, Result};
use crate::journal::{JournalEntry, JournalOp};
use crate::scene::{Scene, SceneAudio, SceneFile};
use crate::{atomic, capture, paths};

pub const FORMAT: &str = "dragonslayer/1";
/// The same format from before the app was renamed from Stopgap. Opened as-is and
/// relabelled `FORMAT` the next time the project is saved.
const LEGACY_FORMATS: [&str; 1] = ["stopgap/1"];
pub const DEFAULT_FPS: u32 = 12;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectFile {
    pub format: String,
    pub name: String,
    pub fps: u32,
    pub scenes: Vec<String>,
    pub active_scene: Option<String>,
    /// Extra takes, by scene id. Take 1 is always the scene itself.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub takes: BTreeMap<String, Takes>,
}

/// The extra takes of one scene and which take the film uses.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Takes {
    /// Take folders in order: take 2, take 3, ...
    pub extra: Vec<String>,
    /// The take in the film. `None` is take 1, the scene itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chosen: Option<String>,
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
                takes: BTreeMap::new(),
            },
        };
        project.add_scene("Scene 1", None)?;
        Ok(project)
    }

    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        let path = root.join(paths::PROJECT_FILE);
        let bytes = fs::read(&path).at(&path)?;
        let mut file: ProjectFile =
            serde_json::from_slice(&bytes).map_err(|source| Error::Json { path, source })?;
        if LEGACY_FORMATS.contains(&file.format.as_str()) {
            file.format = FORMAT.into();
        } else if file.format != FORMAT {
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

    /// A scene or a take of one, by id.
    pub fn scene(&self, id: &str) -> Result<Scene> {
        if !self.file.scenes.iter().any(|s| s == id) && self.take_owner(id).is_none() {
            return Err(Error::SceneNotFound(id.into()));
        }
        Scene::load(paths::scene_dir(&self.root, id))
    }

    /// The scene a take belongs to, if `id` is a take.
    pub fn take_owner(&self, id: &str) -> Option<&str> {
        self.file.takes.iter().find(|(_, t)| t.extra.iter().any(|x| x == id)).map(|(k, _)| k.as_str())
    }

    /// 1 for a scene itself, 2, 3, ... for its extra takes.
    pub fn take_number(&self, id: &str) -> Option<usize> {
        if self.file.scenes.iter().any(|s| s == id) {
            return Some(1);
        }
        let owner = self.take_owner(id)?;
        self.file.takes[owner].extra.iter().position(|x| x == id).map(|i| i + 2)
    }

    /// Extra takes of `scene_id` (not including the scene itself), in order.
    pub fn takes_of(&self, scene_id: &str) -> Result<Vec<Scene>> {
        match self.file.takes.get(scene_id) {
            Some(t) => t.extra.iter().map(|id| self.scene(id)).collect(),
            None => Ok(Vec::new()),
        }
    }

    /// The take of `scene_id` the film uses: the chosen one, or the scene itself.
    pub fn chosen_take<'a>(&'a self, scene_id: &'a str) -> &'a str {
        self.file.takes.get(scene_id).and_then(|t| t.chosen.as_deref()).unwrap_or(scene_id)
    }

    /// What the film is made of: each scene's chosen take, in scene order.
    pub fn film_scenes(&self) -> Result<Vec<Scene>> {
        self.file.scenes.iter().map(|id| self.scene(self.chosen_take(id))).collect()
    }

    /// Every scene and every take (recovery, lookups).
    pub fn all_scenes(&self) -> Result<Vec<Scene>> {
        let mut out = Vec::new();
        for id in &self.file.scenes {
            out.push(self.scene(id)?);
            out.extend(self.takes_of(id)?);
        }
        Ok(out)
    }

    /// Starts a new, empty take of the scene (or of the scene a take belongs to) and makes
    /// it active, so the next captures go into it.
    pub fn add_take(&mut self, id: &str) -> Result<Scene> {
        let owner = match self.take_owner(id) {
            Some(o) => o.to_owned(),
            None => self.scene(id)?.id().to_owned(),
        };
        let base = self.scene(&owner)?;
        let extra = self.file.takes.get(&owner).map_or(0, |t| t.extra.len());
        let mut n = extra + 2;
        let trash = paths::project_trash(&self.root);
        let take_id = loop {
            let candidate = format!("{owner}t{n}");
            if !paths::scene_dir(&self.root, &candidate).exists() && !trash.join(&candidate).exists() {
                break candidate;
            }
            n += 1;
        };
        let scene = Scene::create(
            paths::scene_dir(&self.root, &take_id),
            SceneFile { id: take_id.clone(), name: format!("{} · take {}", base.name(), extra + 2), fps: base.file.fps, audio: None },
        )?;
        self.file.takes.entry(owner).or_default().extra.push(take_id.clone());
        self.file.active_scene = Some(take_id);
        self.save()?;
        Ok(scene)
    }

    /// Picks which take of `scene_id` goes in the film: a take id, or `None` for take 1.
    pub fn choose_take(&mut self, scene_id: &str, take: Option<&str>) -> Result<()> {
        self.index_of(scene_id)?;
        if let Some(t) = take
            && t != scene_id
            && self.take_owner(t) != Some(scene_id)
        {
            return Err(Error::SceneNotFound(t.into()));
        }
        let take = take.filter(|t| *t != scene_id).map(str::to_owned);
        if take.is_none() && !self.file.takes.contains_key(scene_id) {
            return Ok(());
        }
        self.file.takes.entry(scene_id.to_owned()).or_default().chosen = take;
        self.save()
    }

    /// Scenes in project order.
    pub fn scenes(&self) -> Result<Vec<Scene>> {
        self.file.scenes.iter().map(|id| self.scene(id)).collect()
    }

    pub fn active_scene(&self) -> Result<Scene> {
        let id = self.file.active_scene.as_deref().ok_or(Error::NoActiveScene)?;
        self.scene(id)
    }

    /// Resolves a scene or take by id, or by display name if no id matches.
    pub fn find_scene(&self, key: &str) -> Result<Scene> {
        if self.file.scenes.iter().any(|s| s == key) || self.take_owner(key).is_some() {
            return self.scene(key);
        }
        self.all_scenes()?
            .into_iter()
            .find(|s| s.name() == key)
            .ok_or_else(|| Error::SceneNotFound(key.into()))
    }

    pub fn fps_for(&self, scene: &Scene) -> u32 {
        scene.file.fps.unwrap_or(self.file.fps)
    }

    /// Adds a scene after `after` (or at the end) and makes it active.
    pub fn add_scene(&mut self, name: &str, after: Option<&str>) -> Result<Scene> {
        // After a take means after the scene it belongs to.
        let owner = after.and_then(|a| self.take_owner(a)).map(str::to_owned);
        let after = owner.as_deref().or(after);
        let pos = match after {
            Some(a) => {
                self.index_of(a)? + 1
            }
            None => self.file.scenes.len(),
        };
        let id = self.new_scene_id(pos);
        let scene = Scene::create(
            paths::scene_dir(&self.root, &id),
            SceneFile { id: id.clone(), name: name.into(), fps: None, audio: None },
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

    /// Reference audio for a scene or take (a take uses its scene's): the sound file and
    /// the second in it at frame 1.
    pub fn audio_for(&self, scene: &Scene) -> Option<(PathBuf, f64)> {
        let owner = self.take_owner(scene.id()).and_then(|o| self.scene(o).ok());
        let audio = owner.as_ref().unwrap_or(scene).file.audio.as_ref()?;
        Some((self.root.join(&audio.file), audio.start_ms as f64 / 1000.0))
    }

    /// Copies `source` into the project's `audio/` folder and makes it the reference audio
    /// of scene `id` (or of the scene a take belongs to), from its start. `None` removes it
    /// from the scene; the file stays in `audio/`.
    pub fn set_scene_audio(&mut self, id: &str, source: Option<&Path>) -> Result<()> {
        let mut scene = self.scene(self.take_owner(id).unwrap_or(id))?;
        scene.file.audio = match source {
            None => None,
            Some(src) => {
                let dir = paths::audio_dir(&self.root);
                fs::create_dir_all(&dir).at(&dir)?;
                let name = src.file_name().map_or_else(|| "audio".into(), |n| n.to_string_lossy().into_owned());
                let size = |p: &Path| fs::metadata(p).map(|m| m.len()).ok();
                // The same file already in audio/ (picked from there, or added before) is reused.
                let same = dir.join(&name);
                let dest = if size(&same).is_some() && size(&same) == size(src) {
                    same
                } else {
                    let dest = paths::unique_path(&dir, &name);
                    fs::copy(src, &dest).at(src)?;
                    dest
                };
                let file = format!("audio/{}", dest.file_name().unwrap_or_default().to_string_lossy());
                Some(SceneAudio { file, start_ms: 0 })
            }
        };
        scene.save()
    }

    /// Where in its reference audio scene `id` (or its takes) starts: milliseconds at frame 1.
    pub fn set_audio_start(&mut self, id: &str, start_ms: u64) -> Result<()> {
        let mut scene = self.scene(self.take_owner(id).unwrap_or(id))?;
        if let Some(a) = &mut scene.file.audio {
            a.start_ms = start_ms;
            scene.save()?;
        }
        Ok(())
    }

    /// Moves a scene to `to` (0-based, clamped).
    pub fn move_scene(&mut self, id: &str, to: usize) -> Result<()> {
        let from = self.index_of(id)?;
        let id = self.file.scenes.remove(from);
        let to = to.min(self.file.scenes.len());
        self.file.scenes.insert(to, id);
        self.save()
    }

    /// Moves the scene folder (and its takes) into the project `trash/`. Given a take,
    /// removes just that take.
    pub fn delete_scene(&mut self, id: &str) -> Result<()> {
        if let Some(owner) = self.take_owner(id).map(str::to_owned) {
            return self.delete_take(&owner, id);
        }
        let idx = self.index_of(id)?;
        let takes = self.file.takes.remove(id).map(|t| t.extra).unwrap_or_default();
        for t in takes.iter().map(String::as_str).chain(std::iter::once(id)) {
            self.to_trash(t)?;
        }
        self.file.scenes.remove(idx);
        let active_gone = self.file.active_scene.as_deref().is_some_and(|a| a == id || takes.iter().any(|t| t == a));
        if active_gone {
            let next = idx.min(self.file.scenes.len().saturating_sub(1));
            self.file.active_scene = self.file.scenes.get(next).cloned();
        }
        self.save()
    }

    fn delete_take(&mut self, owner: &str, take: &str) -> Result<()> {
        self.to_trash(take)?;
        if let Some(t) = self.file.takes.get_mut(owner) {
            t.extra.retain(|x| x != take);
            if t.chosen.as_deref() == Some(take) {
                t.chosen = None;
            }
            if t.extra.is_empty() {
                self.file.takes.remove(owner);
            }
        }
        if self.file.active_scene.as_deref() == Some(take) {
            self.file.active_scene = Some(owner.to_owned());
        }
        self.save()
    }

    fn to_trash(&self, id: &str) -> Result<()> {
        let src = paths::scene_dir(&self.root, id);
        let trash = paths::project_trash(&self.root);
        fs::create_dir_all(&trash).at(&trash)?;
        let dest = paths::unique_path(&trash, id);
        fs::rename(&src, &dest).at(&src)
    }

    /// Makes a scene or a take the one captures go into.
    pub fn set_active(&mut self, id: &str) -> Result<()> {
        if self.take_owner(id).is_none() {
            self.index_of(id)?;
        }
        self.file.active_scene = Some(id.into());
        self.save()
    }

    /// Commits or abandons captures left pending by a crash. Run on launch.
    pub fn recover(&self) -> Result<RecoveryReport> {
        let mut report = RecoveryReport::default();
        for scene in self.all_scenes()? {
            let pending = scene.pending()?;
            if pending.is_empty() {
                continue;
            }
            let journal = scene.journal()?;
            for frame in pending {
                let files = list_files(&capture::pending_dir(&scene, &frame))?;
                match files {
                    files if !files.is_empty() => {
                        // Keep an interrupted import's source so a re-run still skips it.
                        let source = journal
                            .iter()
                            .rev()
                            .find(|e| e.op == JournalOp::Pending && e.frame == frame)
                            .and_then(|e| e.source.clone());
                        capture::commit(&scene, &frame, &files, None, source)?;
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
