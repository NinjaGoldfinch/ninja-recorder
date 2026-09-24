//! Capture backend abstraction. Every recording implementation — the real
//! libobs backend (Windows) and the stub used everywhere else —
//! lives behind this trait. Nothing above this module may depend on libobs
//! types directly; see DEVELOPMENT.md §2.2.

pub mod audio;
pub mod backend;
pub mod devices;
#[cfg(target_os = "windows")]
pub mod libobs;
// The target backend (Option B). Empty until WS1 task 1.6; gated to Windows
// for the same reason `libobs` is, so the Linux and macOS dev loops keep
// compiling exactly what they compile today.
#[cfg(target_os = "windows")]
pub mod own;
// The faststart remux. Not behind a backend's `cfg`: startup recovery
// remuxes a killed recording whichever backend wrote it.
pub mod remux;
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

impl RecordConfig {
    /// Where the backend is expected to write, so that a caller can name the
    /// file before it exists.
    ///
    /// **A prediction, not a promise.** The trait lets a backend choose its
    /// own container, and only `Recorder::stop`'s `RecordingOutput::path`
    /// says what was actually written. Both shipped backends derive the path
    /// exactly this way and call this method to do it, which is the point:
    /// the `{stem}.mp4` rule now lives in one place instead of three.
    ///
    /// The supervisor uses it to insert a `recordings` row when recording
    /// starts (#150), so markers have somewhere to go before the finalize a
    /// killed daemon never reaches. That row is corrected **by id** at
    /// finalize, from `stop`'s path, so a backend that writes somewhere else
    /// costs a row that is briefly wrong about its own filename and nothing
    /// more.
    pub fn expected_output_path(&self) -> PathBuf {
        self.output_dir.join(format!("{}.mp4", self.file_stem))
    }
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
    /// is decided at runtime (`daemon::backends`, through `backend::choose`)
    /// by target OS, by the `capture_backend` setting *and* by whether the
    /// chosen backend can be built, so it can't be inferred from
    /// `cfg!` at the call site — the dev portal and any future
    /// user-facing "recording unavailable" message both need to ask the
    /// object itself. `FailedRecorder` folds its init error in here,
    /// which is why this returns an owned `String`.
    fn backend_name(&self) -> String;

    /// Bring the backend up ahead of time, because a recording now looks
    /// plausible — the supervisor calls this on entering `ClientRunning`.
    ///
    /// Purely a pre-warm: `start` brings the backend up itself if this was
    /// never called or failed, so nothing depends on it having run. That
    /// matters because the two can race — a client that goes straight into
    /// a game gets `prepare` and `start` back to back — and correctness
    /// must not hinge on which wins.
    ///
    /// Default no-op: only a backend with expensive resident state needs
    /// to care.
    fn prepare(&mut self) -> Result<(), RecorderError> {
        Ok(())
    }

    /// Drop whatever `prepare` acquired, because no game is plausible any
    /// more — the supervisor calls this on returning to `Idle`.
    ///
    /// Must be a no-op while a recording is in flight, and infallible: it
    /// runs on a path where there is nothing useful to do with an error.
    fn release(&mut self) {}

    /// Collect whatever diagnostic output the backend has queued since it was
    /// last asked — the supervisor calls this every few Live Client polls
    /// while a recording runs.
    ///
    /// For the libobs backend this is the whole of how the worker's output
    /// moves mid-game: its info and warnings come up the IPC pipe and are
    /// only read while a command waits for its reply, so a long recording
    /// with no commands leaves them sitting in the pipe (#221).
    ///
    /// Infallible and best-effort, like `release`: it must never be the
    /// reason a recording stops. Default no-op.
    fn collect_output(&mut self) {}
}

/// A backend that refuses, and says why.
///
/// Three callers. It stands in for the real backend when that cannot be
/// brought up (Windows only, see `libobs::LibObsRecorder::new`), because
/// startup must not fail just because capture is unavailable: LCU polling, the
/// VOD library and the review UI do not depend on it, so the app should open
/// and surface the original error only if the user tries to record.
///
/// And it is what the **UI process** holds, where refusing is the correct
/// behaviour rather than a degraded one: that process does not record, and
/// `StubRecorder` would fabricate a file instead of saying so.
///
/// And since WS1.7 it is what the `capture_backend` setting gets when it names
/// a backend this build cannot construct (`backend::construct`), which is the
/// own backend until WS1.6 lands it. Refused with the reason, never recorded
/// on the other backend in its place.
pub struct FailedRecorder(pub String);

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
