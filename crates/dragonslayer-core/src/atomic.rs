use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::Path;

use crate::error::{IoContext, Result};

/// Temp file in the same directory, flush to disk, rename over the target.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path.parent().expect("path has a parent");
    let name = path.file_name().expect("path has a file name").to_string_lossy();
    let tmp = dir.join(format!(".{name}.tmp"));
    {
        let mut f = File::create(&tmp).at(&tmp)?;
        f.write_all(bytes).at(&tmp)?;
        f.sync_all().at(&tmp)?;
    }
    fs::rename(&tmp, path).at(path)?;
    sync_dir(dir);
    Ok(())
}

pub fn write_json_atomic<T: serde::Serialize>(path: &Path, value: &T) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value).expect("model serialises");
    bytes.push(b'\n');
    write_atomic(path, &bytes)
}

pub fn fsync_file(path: &Path) -> Result<()> {
    // Windows needs write access for FlushFileBuffers.
    let f = OpenOptions::new().read(true).write(true).open(path).at(path)?;
    f.sync_all().at(path)
}

/// Persist a rename/create in `dir`. Only meaningful on Unix; NTFS journals metadata itself.
pub fn sync_dir(dir: &Path) {
    #[cfg(unix)]
    if let Ok(d) = File::open(dir) {
        let _ = d.sync_all();
    }
    #[cfg(not(unix))]
    let _ = dir;
}
