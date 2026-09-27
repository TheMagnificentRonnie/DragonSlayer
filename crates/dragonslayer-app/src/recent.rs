//! Remembers your projects, and whether the app closed cleanly so the welcome screen can
//! offer "continue where you left off" (and say so plainly after a crash or power cut).
//!
//! Built so the list can't lose projects:
//! - Saving merges with what's already on disk, so two copies of the app open at once
//!   can't overwrite each other's entries.
//! - The previous file is kept as `recent.json.bak` and used if the main one is damaged.
//! - Missing projects (a USB stick that isn't plugged in) stay listed until you forget them.
//! - `find_projects` searches your folders for projects the list never knew about.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

const KEEP: usize = 100;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recent {
    /// Most recent first.
    #[serde(default)]
    pub projects: Vec<PathBuf>,
    /// False from opening a project until the window closes normally. Still false on the
    /// next launch means the app crashed, was killed, or lost power.
    #[serde(default = "yes")]
    pub clean_exit: bool,
}

fn yes() -> bool {
    true
}

impl Default for Recent {
    fn default() -> Self {
        Self { projects: Vec::new(), clean_exit: true }
    }
}

impl Recent {
    pub fn load() -> Self {
        let Some(path) = file() else { return Self::default() };
        read(&path).or_else(|| read(&backup(&path))).unwrap_or_default()
    }

    /// Records `project` as the one being worked on (and not yet cleanly closed).
    pub fn opened(&mut self, project: &Path) {
        // absolute, not canonicalize: that gives \\?\C:\... paths on Windows.
        let project = std::path::absolute(project).unwrap_or_else(|_| project.to_owned());
        self.push(project);
        self.clean_exit = false;
        self.save(true);
    }

    pub fn closed_cleanly(&mut self) {
        self.clean_exit = true;
        self.save(true);
    }

    /// Adds projects found on disk, after the ones already listed. Returns how many were new.
    pub fn add_found(&mut self, found: &[PathBuf]) -> usize {
        let before = self.projects.len();
        for p in found {
            if !self.projects.iter().any(|q| same(q, p)) {
                self.projects.push(p.clone());
            }
        }
        let added = self.projects.len() - before;
        if added > 0 {
            self.save(true);
        }
        added
    }

    /// Removes a project from the list (the folder itself is untouched).
    pub fn forget(&mut self, project: &Path) {
        self.projects.retain(|p| !same(p, project));
        self.save(false);
    }

    fn push(&mut self, project: PathBuf) {
        self.projects.retain(|p| !same(p, &project));
        self.projects.insert(0, project);
        self.projects.truncate(KEEP);
    }

    /// `merge`: keep entries another copy of the app added since we loaded.
    fn save(&mut self, merge: bool) {
        let Some(path) = file() else { return };
        if merge && let Some(disk) = read(&path) {
            for p in disk.projects {
                if !self.projects.iter().any(|q| same(q, &p)) {
                    self.projects.push(p);
                }
            }
            self.projects.truncate(KEEP);
        }
        write(&path, self);
    }
}

fn read(path: &Path) -> Option<Recent> {
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

fn write(path: &Path, r: &Recent) {
    let Some(dir) = path.parent() else { return };
    if fs::create_dir_all(dir).is_err() {
        return;
    }
    let Ok(bytes) = serde_json::to_vec_pretty(r) else { return };
    // Keep the last good file as a backup before replacing it.
    if read(path).is_some() {
        let _ = fs::copy(path, backup(path));
    }
    let tmp = path.with_extension("json.tmp");
    if fs::write(&tmp, bytes).is_ok() {
        let _ = fs::rename(&tmp, path);
    }
}

fn backup(path: &Path) -> PathBuf {
    path.with_extension("json.bak")
}

fn same(a: &Path, b: &Path) -> bool {
    if cfg!(any(windows, target_os = "macos")) {
        a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy())
    } else {
        a == b
    }
}

/// `%APPDATA%\DragonSlayer\recent.json`, `~/Library/Application Support/DragonSlayer/recent.json`,
/// or `$XDG_CONFIG_HOME/dragonslayer/recent.json`.
fn file() -> Option<PathBuf> {
    // Tests must never touch the user's real list.
    if cfg!(test) {
        return Some(std::env::temp_dir().join(format!("dragonslayer-test-{}", std::process::id())).join("recent.json"));
    }
    let dir = if cfg!(windows) {
        PathBuf::from(std::env::var_os("APPDATA")?).join("DragonSlayer")
    } else if cfg!(target_os = "macos") {
        PathBuf::from(std::env::var_os("HOME")?).join("Library/Application Support/DragonSlayer")
    } else {
        match std::env::var_os("XDG_CONFIG_HOME") {
            Some(d) => PathBuf::from(d).join("dragonslayer"),
            None => PathBuf::from(std::env::var_os("HOME")?).join(".config/dragonslayer"),
        }
    };
    Some(dir.join("recent.json"))
}

/// Where new projects go by default: `Videos\DragonSlayer`. Not Documents: on many
/// Windows machines that's synced by OneDrive, which would upload ~60 MB per frame.
pub fn default_projects_dir() -> Option<PathBuf> {
    dirs::video_dir().or_else(dirs::home_dir).map(|d| d.join("DragonSlayer"))
}

/// Places worth searching for projects, with how deep to look: your home folder (which
/// includes Desktop, Documents and OneDrive) and the root of every other drive, where
/// people make "Films" folders and where camera cards and USB sticks show up.
pub fn search_roots() -> Vec<(PathBuf, usize)> {
    let mut roots = Vec::new();
    if let Some(d) = default_projects_dir() {
        roots.push((d, 2));
    }
    for d in [dirs::video_dir(), dirs::document_dir(), dirs::desktop_dir(), dirs::picture_dir()].into_iter().flatten() {
        roots.push((d, 4));
    }
    if let Some(home) = dirs::home_dir() {
        roots.push((home, 5));
    }
    if cfg!(windows) {
        for letter in b'D'..=b'Z' {
            let root = PathBuf::from(format!("{}:\\", letter as char));
            if root.is_dir() {
                roots.push((root, 3));
            }
        }
    } else if cfg!(target_os = "macos")
        && let Ok(vols) = fs::read_dir("/Volumes")
    {
        roots.extend(vols.flatten().map(|v| (v.path(), 3)));
    }
    roots
}

/// Every DragonSlayer project under `roots`, stopping after `budget` so a huge drive can't
/// hang the search. Skips system folders, hidden folders and the insides of projects.
pub fn find_projects(roots: &[(PathBuf, usize)], budget: Duration) -> Vec<PathBuf> {
    let deadline = Instant::now() + budget;
    let mut found: Vec<PathBuf> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (root, depth) in roots {
        walk(root, *depth, deadline, &mut found, &mut seen);
    }
    found
}

fn walk(dir: &Path, depth: usize, deadline: Instant, found: &mut Vec<PathBuf>, seen: &mut std::collections::HashSet<String>) {
    if Instant::now() > deadline || !seen.insert(dir.to_string_lossy().to_lowercase()) {
        return;
    }
    // Keep going inside a project too: people do end up making one inside another.
    if dragonslayer_core::project::is_project(dir) && dragonslayer_core::Project::open(dir).is_ok() {
        found.push(dir.to_owned());
    }
    if depth == 0 {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let Ok(t) = e.file_type() else { continue };
        if !t.is_dir() || t.is_symlink() {
            continue;
        }
        let name = e.file_name().to_string_lossy().into_owned();
        // Inside projects: frames live in scenes/, deleted ones in trash/, videos in exports/.
        const SKIP: [&str; 15] = [
            "AppData", "node_modules", "target", "Windows", "Program Files", "Program Files (x86)",
            "ProgramData", "$Recycle.Bin", "System Volume Information", "Library", "Applications",
            "msys64", "scenes", "exports", "trash",
        ];
        if name.starts_with('.') || SKIP.iter().any(|s| s.eq_ignore_ascii_case(&name)) {
            continue;
        }
        walk(&e.path(), depth - 1, deadline, found, seen);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn most_recent_first_no_duplicates_and_capped() {
        let mut r = Recent::default();
        for i in 0..(KEEP + 10) {
            r.push(PathBuf::from(format!("/films/f{i}")));
        }
        r.push(PathBuf::from("/films/f103"));
        assert_eq!(r.projects.len(), KEEP);
        assert_eq!(r.projects[0], PathBuf::from("/films/f103"));
        assert_eq!(r.projects.iter().filter(|p| p.ends_with("f103")).count(), 1);
        assert_eq!(r.projects[1], PathBuf::from(format!("/films/f{}", KEEP + 9)));
    }

    #[test]
    fn a_file_from_an_older_version_or_a_torn_write_still_loads() {
        let r: Recent = serde_json::from_str(r#"{"projects": ["/a"]}"#).unwrap();
        assert!(r.clean_exit, "missing flag means no crash to report");
        assert!(serde_json::from_str::<Recent>("{\"projects\": [\"/a\"").is_err());
    }

    /// Everything that touches the (test) recent file, in one test so parallel tests can't race.
    #[test]
    fn saving_merges_backs_up_and_recovers() {
        let path = file().unwrap();
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(backup(&path));

        // Two copies of the app, each opening a different project.
        let mut a = Recent::load();
        let mut b = Recent::load();
        a.opened(Path::new("/films/one"));
        b.opened(Path::new("/films/two"));
        let on_disk = Recent::load();
        assert!(on_disk.projects.iter().any(|p| p.ends_with("one")), "{:?}", on_disk.projects);
        assert!(on_disk.projects.iter().any(|p| p.ends_with("two")), "{:?}", on_disk.projects);

        // A damaged main file falls back to the backup instead of an empty list.
        fs::write(&path, b"{\"projects\": [").unwrap();
        let recovered = Recent::load();
        assert!(!recovered.projects.is_empty(), "backup should be used");

        // Forgetting sticks even though saving normally merges.
        let mut r = Recent::load();
        r.forget(Path::new(&r.projects[0].clone()));
        let n = r.projects.len();
        assert_eq!(Recent::load().projects.len(), n);

        // Found projects go after the listed ones, without duplicates.
        let mut r = Recent::load();
        r.opened(Path::new("/films/three"));
        let first = r.projects[0].clone();
        let added = r.add_found(&[first.clone(), PathBuf::from("/films/found")]);
        assert_eq!(added, 1);
        assert_eq!(Recent::load().projects.last().unwrap(), &PathBuf::from("/films/found"));
    }

    #[test]
    fn finds_projects_skips_insides_and_system_folders() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        dragonslayer_core::Project::create(root.join("Films/Hannah2"), "Hannah2", 12).unwrap();
        dragonslayer_core::Project::create(root.join("deep/a/b/Chase"), "Chase", 12).unwrap();
        // Made inside another project by accident.
        dragonslayer_core::Project::create(root.join("Films/Hannah2/xxx"), "xxx", 12).unwrap();
        dragonslayer_core::Project::create(root.join("AppData/Nope"), "Nope", 12).unwrap();
        dragonslayer_core::Project::create(root.join(".hidden/Nope2"), "Nope2", 12).unwrap();
        // A folder with a project.json that isn't ours.
        fs::create_dir_all(root.join("web")).unwrap();
        fs::write(root.join("web/project.json"), b"{\"name\":\"a website\"}").unwrap();

        let found = find_projects(&[(root.to_owned(), 5)], Duration::from_secs(10));
        let mut names: Vec<_> = found.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        names.sort();
        assert_eq!(names, ["Chase", "Hannah2", "xxx"]);

        // Depth limit.
        let shallow = find_projects(&[(root.to_owned(), 2)], Duration::from_secs(10));
        assert_eq!(shallow.len(), 1);
    }
}

#[cfg(test)]
mod real_machine {
    /// cargo test -p dragonslayer-app --release -- --ignored --nocapture find_on_this_machine
    #[test]
    #[ignore = "searches this computer's real folders"]
    fn find_on_this_machine() {
        let roots = super::search_roots();
        let t = std::time::Instant::now();
        let found = super::find_projects(&roots, std::time::Duration::from_secs(30));
        println!("roots: {:?}", roots.iter().map(|(p, d)| format!("{} ({d})", p.display())).collect::<Vec<_>>());
        println!("found {} projects in {:?}", found.len(), t.elapsed());
        for p in found {
            println!("  {}", p.display());
        }
    }
}
