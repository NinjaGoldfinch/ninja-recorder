//! Capture backend abstraction. Every recording implementation — the real
//! libobs backend (Windows, Phase 6) and the stub used everywhere else —
//! lives behind this trait. Nothing above this module may depend on libobs
//! types directly; see DEVELOPMENT.md §2.2.

#[cfg(target_os = "windows")]
pub mod libobs;
// Also compiled on Windows under `cfg(test)`: `state_machine::supervisor`'s
// unit tests use `StubRecorder` as a platform-agnostic dummy `Recorder`
// regardless of which real backend the current platform ships.
#[cfg(any(test, not(target_os = "windows")))]
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
    /// Capture-backend failure that isn't one of the above — libobs/IPC
    /// errors, no usable hardware encoder found, etc. Carries a message
    /// rather than the backend's own error type so this enum (and every
    /// caller matching on it) stays libobs-free per this module's header.
    /// Only constructed by the Windows backend — `StubRecorder` never
    /// fails this way — so it's legitimately dead code on every other
    /// platform.
    #[error("recorder backend error: {0}")]
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    Backend(String),
}

pub trait Recorder: Send {
    fn start(&mut self, config: RecordConfig) -> Result<(), RecorderError>;
    /// Finalizes the recording and returns the path to the resulting file.
    fn stop(&mut self) -> Result<PathBuf, RecorderError>;
    fn is_recording(&self) -> bool;
}
