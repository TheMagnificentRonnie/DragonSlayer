//! DragonSlayer core: project and scene model, on-disk format, journal,
//! the capture-commit transaction and compile. No camera or UI code.

mod atomic;
pub mod capture;
pub mod compile;
mod error;
pub mod import;
pub mod journal;
pub mod paths;
pub mod project;
pub mod scene;

pub use error::{Error, Result};
pub use journal::{JournalEntry, JournalOp};
pub use project::{Project, ProjectFile, RecoveryReport};
pub use scene::{Frame, Scene, SceneFile};
