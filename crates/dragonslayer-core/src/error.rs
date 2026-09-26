use std::path::PathBuf;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("{path}: invalid JSON: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error("project format {found:?} is not supported (expected {expected:?})")]
    UnsupportedFormat { found: String, expected: &'static str },

    #[error("scene {0:?} not found")]
    SceneNotFound(String),

    #[error("no active scene; add one first")]
    NoActiveScene,

    #[error("scene {0:?} has no frames")]
    NoFrames(String),

    #[error("a project already exists at {0}")]
    ProjectExists(PathBuf),

    #[error("not enough disk space: {available} bytes free, need at least {required}")]
    DiskFull { available: u64, required: u64 },

    #[error("the camera returned no files")]
    NothingDownloaded,

    #[error("capture failed: {0}")]
    Capture(String),

    #[error("nothing to compile: {0}")]
    NothingToCompile(String),

    #[error("ffmpeg: {0}")]
    Ffmpeg(String),
}

impl Error {
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io { path: path.into(), source }
    }
}

pub(crate) trait IoContext<T> {
    fn at(self, path: impl Into<PathBuf>) -> Result<T>;
}

impl<T> IoContext<T> for std::io::Result<T> {
    fn at(self, path: impl Into<PathBuf>) -> Result<T> {
        self.map_err(|e| Error::io(path, e))
    }
}
