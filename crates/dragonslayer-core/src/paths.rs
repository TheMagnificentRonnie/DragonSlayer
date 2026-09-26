use std::path::{Path, PathBuf};

pub const PROJECT_FILE: &str = "project.json";
pub const SCENE_FILE: &str = "scene.json";
pub const JOURNAL_FILE: &str = "journal.ndjson";

pub fn scenes_dir(root: &Path) -> PathBuf {
    root.join("scenes")
}

pub fn scene_dir(root: &Path, id: &str) -> PathBuf {
    scenes_dir(root).join(id)
}

pub fn project_trash(root: &Path) -> PathBuf {
    root.join("trash")
}

pub fn exports_dir(root: &Path) -> PathBuf {
    root.join("exports")
}

pub fn frames_dir(scene_dir: &Path) -> PathBuf {
    scene_dir.join("frames")
}

pub fn incoming_dir(scene_dir: &Path) -> PathBuf {
    scene_dir.join("incoming")
}

pub fn scene_trash(scene_dir: &Path) -> PathBuf {
    scene_dir.join("trash")
}

/// Returns `dir/name`, or `dir/<stem>-2.<ext>`, `-3`, ... if that already exists.
pub fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let candidate = dir.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s, Some(e)),
        _ => (name, None),
    };
    (2..)
        .map(|n| match ext {
            Some(e) => dir.join(format!("{stem}-{n}.{e}")),
            None => dir.join(format!("{stem}-{n}")),
        })
        .find(|p| !p.exists())
        .expect("unbounded search")
}
