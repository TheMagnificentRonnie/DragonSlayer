use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::error::{IoContext, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JournalOp {
    /// Capture started; files may be sitting in `incoming/`.
    Pending,
    /// Frame committed into `frames/`.
    Capture,
    /// Pending capture that recovery found no files for.
    Abandon,
    /// Frame moved to the scene's `trash/`.
    Delete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalEntry {
    #[serde(with = "time::serde::rfc3339")]
    pub t: OffsetDateTime,
    pub op: JournalOp,
    pub frame: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera: Option<String>,
}

impl JournalEntry {
    pub fn now(op: JournalOp, frame: impl Into<String>, camera: Option<String>) -> Self {
        let t = OffsetDateTime::now_utc().replace_nanosecond(0).expect("0 is valid");
        Self { t, op, frame: frame.into(), camera }
    }
}

/// Reads every parseable line. A torn final line (crash mid-append) is skipped.
pub fn read(path: &Path) -> Result<Vec<JournalEntry>> {
    let f = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(crate::Error::io(path, e)),
    };
    let mut out = Vec::new();
    for line in BufReader::new(f).lines() {
        let line = line.at(path)?;
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(entry) = serde_json::from_str(&line) {
            out.push(entry);
        }
    }
    Ok(out)
}

/// Appends one line and flushes it to disk before returning.
pub fn append(path: &Path, entry: &JournalEntry) -> Result<()> {
    let mut line = serde_json::to_string(entry).expect("entry serialises");
    line.push('\n');
    let mut f = OpenOptions::new().create(true).append(true).open(path).at(path)?;
    // Terminate a torn previous line so this entry stays parseable.
    if f.metadata().at(path)?.len() > 0 && !ends_with_newline(path)? {
        line.insert(0, '\n');
    }
    f.write_all(line.as_bytes()).at(path)?;
    f.sync_all().at(path)
}

fn ends_with_newline(path: &Path) -> Result<bool> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = File::open(path).at(path)?;
    f.seek(SeekFrom::End(-1)).at(path)?;
    let mut b = [0u8; 1];
    f.read_exact(&mut b).at(path)?;
    Ok(b[0] == b'\n')
}
