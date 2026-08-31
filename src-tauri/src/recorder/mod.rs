//! Capture backend abstraction. Every recording implementation — the real
//! libobs backend (Windows, Phase 6) and the stub used everywhere else —
//! lives behind this trait. Nothing above this module may depend on libobs
//! types directly; see DEVELOPMENT.md §2.2.

pub mod stub;

use std::path::PathBuf;

/// Parameters for a single recording. Intentionally minimal for Phase 1 —
/// resolution/encoder/audio-source options land alongside the libobs backend.
#[derive(Debug, Clone)]
pub struct RecordConfig {
    pub output_dir: PathBuf,
    /// Filename without extension; the backend chooses the container.
    pub file_stem: String,
}

#[derive(Debug, thiserror::Error)]
pub enum RecorderError {
    #[error("recorder is already recording")]
    AlreadyRecording,
    #[error("recorder is not currently recording")]
    NotRecording,
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

pub trait Recorder: Send {
    fn start(&mut self, config: RecordConfig) -> Result<(), RecorderError>;
    /// Finalizes the recording and returns the path to the resulting file.
    fn stop(&mut self) -> Result<PathBuf, RecorderError>;
    fn is_recording(&self) -> bool;
}
