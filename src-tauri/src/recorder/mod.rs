//! Capture backend abstraction. Every recording implementation — the real
//! libobs backend (Windows) and the stub used everywhere else —
//! lives behind this trait. Nothing above this module may depend on libobs
//! types directly; see DEVELOPMENT.md §2.2.

pub mod audio;
pub mod devices;
#[cfg(target_os = "windows")]
pub mod libobs;
// Also compiled on Windows under `cfg(test)`: `state_machine::supervisor`'s
// unit tests use `StubRecorder` as a platform-agnostic dummy `Recorder`
// regardless of which real backend the current platform ships.
#[cfg(any(test, not(target_os = "windows")))]
pub mod stub;

use audio::{AudioLayout, AudioPreset};
use std::path::PathBuf;

/// Parameters for a single recording. Still minimal: resolution and encoder
/// are the backend's business, not the caller's. Audio is the exception —
/// what gets captured is a user-facing product decision (whose microphone,
/// on which track), so it's chosen above the trait and passed down. See
/// DEVELOPMENT.md §2.5.
#[derive(Debug, Clone, Default)]
pub struct RecordConfig {
    pub output_dir: PathBuf,
    /// Filename without extension; the backend chooses the container.
    pub file_stem: String,
    /// Only the libobs backend acts on this — `StubRecorder` captures
    /// nothing — so it's legitimately unread on every other platform, same
    /// as `RecorderError::Backend` below.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub audio: AudioPreset,
}

/// What `Recorder::stop` produced.
///
/// The track layout is reported by the backend rather than assumed from the
/// `RecordConfig` that started the recording: a microphone can be unplugged
/// between the settings screen and the end of the game, and the row we write
/// has to describe the file that actually exists.
#[derive(Debug, Clone)]
pub struct RecordingOutput {
    pub path: PathBuf,
    pub audio: AudioLayout,
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
    /// Finalizes the recording and reports the file it produced, along
    /// with the audio track layout that actually made it into that file.
    fn stop(&mut self) -> Result<RecordingOutput, RecorderError>;
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

    fn stop(&mut self) -> Result<RecordingOutput, RecorderError> {
        Err(RecorderError::NotRecording)
    }

    fn is_recording(&self) -> bool {
        false
    }

    fn backend_name(&self) -> String {
        format!("unavailable ({})", self.0)
    }
}
