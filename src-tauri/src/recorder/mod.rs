//! Capture backend abstraction. Every recording implementation — the real
//! libobs backend (Windows) and the stub used everywhere else —
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

/// Parameters for a single recording. Intentionally minimal: resolution,
/// encoder and audio-source choices are the backend's, not the caller's.
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
    /// Which backend is actually live, for diagnostics. Which one you get
    /// is decided at runtime (`lib.rs`'s `setup`) by target OS *and* by
    /// whether libobs managed to initialize, so it can't be inferred from
    /// `cfg!` at the call site — the dev portal and any future
    /// user-facing "recording unavailable" message both need to ask the
    /// object itself. `FailedRecorder` folds its init error in here,
    /// which is why this returns an owned `String`.
    fn backend_name(&self) -> String;
}

/// Stands in for the real backend when it fails to initialize (Windows
/// only — see `libobs::LibObsRecorder::new`). Startup must not fail just
/// because capture is unavailable: LCU polling, the VOD library, and the
/// review UI don't depend on it, so the app should still open and only
/// surface the original error if/when the user tries to record.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub struct FailedRecorder(pub String);

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
impl Recorder for FailedRecorder {
    fn start(&mut self, _config: RecordConfig) -> Result<(), RecorderError> {
        Err(RecorderError::Backend(self.0.clone()))
    }

    fn stop(&mut self) -> Result<PathBuf, RecorderError> {
        Err(RecorderError::NotRecording)
    }

    fn is_recording(&self) -> bool {
        false
    }

    fn backend_name(&self) -> String {
        format!("unavailable ({})", self.0)
    }
}
