//! Everything in the own backend that calls Windows: WGC capture, the D3D11
//! texture path, process-loopback audio, and Media Foundation encoding. Only
//! this module is gated to Windows; the decisions it acts on live beside it
//! in `clock`, `feed`, `root`, `select` and `status`, which compile and are
//! tested everywhere.
//!
//! - `device` — the Windows build, the adapters, the D3D11 device, QPC.
//! - `capture` — the WGC capture of the game window, and the slots its
//!   frames are copied into.
//! - `scale` — frames into the fixed-size slots: a copy, or the D3D11 video
//!   processor scaling a resized window into them, letterboxed.
//! - `process` — the process table and the game window's owner, which
//!   `root` chooses the audio's process tree from.
//! - `audio` — every audio source on its own thread (the game and
//!   applications by process loopback, the microphone and the desktop from
//!   their endpoints), and track 0, the mix of them.
//! - `encode` — the H.264 encoders on offer, and the sink writer with its
//!   AAC stream.
//! - `session` — the thread that owns all of the above.
//!
//! [`OwnRecorder`] is the `Recorder` the daemon builds. It owns no COM object:
//! it drives the session thread over a channel.

mod audio;
mod capture;
mod device;
mod encode;
mod process;
mod scale;
mod session;

use std::path::PathBuf;
use std::sync::mpsc::{RecvTimeoutError, Sender, channel};
use std::thread::JoinHandle;
use std::time::Duration;

use super::status::Status;
use super::{plan, select};
use crate::recorder::audio::AudioLayout;
use crate::recorder::{RecordConfig, Recorder, RecorderError, RecordingOutput};
use crate::{info, warn};
use session::{Command, Started};

pub use device::windows_build;

/// How long `stop` waits for the session thread to finalize. A clean
/// finalize drains a few frames and writes one fragment, well under a second;
/// this is for one that never returns.
const STOP_WAIT: Duration = Duration::from_secs(20);

/// The own capture backend (Option B): WGC → D3D11 → Media Foundation.
///
/// **`Send`, and holding no COM object.** Every D3D, Media Foundation and
/// WinRT object lives on the session thread (`session.rs`), which this
/// drives over a channel and waits on for each answer. The supervisor keeps
/// this in a `Mutex<Box<dyn Recorder>>` and calls it from whichever thread
/// it is on, which is fine for a sender and a join handle and would not be
/// for an apartment-bound COM pointer. The same boundary becomes the capture
/// worker's pipe in WS1.6.9 (#241).
///
/// Every audio preset since #238, mixed into track 0; the stems arrive with
/// #239. Reachable only from a devtools build that selects it
/// (DEVELOPMENT.md §16, "The switch, and when it applies").
pub struct OwnRecorder {
    session: Option<SessionThread>,
    status: Status,
    /// The recording in flight.
    active: Option<Active>,
    /// For the faststart remux on stop; `None` skips it, as for libobs.
    ffmpeg_path: Option<PathBuf>,
}

/// What `stop` reports, fixed when the recording starts: the file, and the
/// audio layout that made it into it.
struct Active {
    path: PathBuf,
    /// What the file holds: track 0, the mix, over every source that opened
    /// (`plan::realised_layout`), or no tracks if none did. The track exists
    /// from `start` to the finalize either way: a source that dies
    /// mid-recording leaves it padded with silence, not missing.
    audio: AudioLayout,
}

struct SessionThread {
    commands: Sender<Command>,
    thread: JoinHandle<()>,
}

impl OwnRecorder {
    /// Cheap and infallible, like `LibObsRecorder::new`: nothing comes up
    /// until `prepare` or `start`.
    pub fn new(ffmpeg_path: Option<PathBuf>) -> Self {
        Self { session: None, status: Status::Idle, active: None, ffmpeg_path }
    }

    /// Sends `command` to the session thread, starting it if it is not
    /// running, and waits for the answer on the channel `make` was given.
    fn ask<T>(
        &mut self,
        make: impl FnOnce(Sender<T>) -> Command,
    ) -> Result<T, RecorderError> {
        self.ask_within(None, make)
    }

    /// [`OwnRecorder::ask`], giving up after `wait` if there is one. A thread
    /// that has not answered by then is left behind, still running, and the
    /// next command starts a new one.
    fn ask_within<T>(
        &mut self,
        wait: Option<Duration>,
        make: impl FnOnce(Sender<T>) -> Command,
    ) -> Result<T, RecorderError> {
        if self.session.is_none() {
            let (commands, receiver) = channel();
            let thread = std::thread::Builder::new()
                .name("own-capture".to_string())
                .spawn(move || session::run(receiver))
                .map_err(|e| {
                    RecorderError::Backend(format!("could not start the capture thread: {e}"))
                })?;
            self.session = Some(SessionThread { commands, thread });
        }
        let (reply, answer) = channel();
        let session = self.session.as_ref().expect("just started");
        if session.commands.send(make(reply)).is_err() {
            self.session = None;
            return Err(RecorderError::Backend("the capture thread has exited".to_string()));
        }
        let answered = match wait {
            None => answer.recv().map_err(|_| RecvTimeoutError::Disconnected),
            Some(wait) => answer.recv_timeout(wait),
        };
        answered.map_err(|e| {
            self.session = None;
            RecorderError::Backend(match e {
                RecvTimeoutError::Disconnected => {
                    "the capture thread exited before answering".to_string()
                }
                RecvTimeoutError::Timeout => format!(
                    "the capture thread did not answer within {} s, and was left behind",
                    wait.unwrap_or_default().as_secs()
                ),
            })
        })
    }

    /// Ends the session thread, finalizing anything it is still recording,
    /// and waits for it: every COM object is released by the time this
    /// returns.
    fn tear_down(&mut self) {
        if let Some(session) = self.session.take() {
            let _ = session.commands.send(Command::Release);
            if session.thread.join().is_err() {
                warn!("recorder", "own backend: the capture thread panicked");
            }
        }
        self.status = Status::Idle;
    }
}

impl Recorder for OwnRecorder {
    /// Starts recording the game window into `config`'s `.mp4`.
    ///
    /// **Video time zero is the last thing this does.** The session thread
    /// finds the window, waits (about 3 s at most) for a real size and WGC's
    /// first frame, and brings the encoder up; only then does this read the
    /// performance counter and hand that instant over as the origin of the
    /// tick grid. Tick 0 of the file is that instant, and it is taken
    /// immediately before `start` returns, which is immediately before the
    /// supervisor stamps `record_started_at`. So the file's t = 0 and the
    /// supervisor's clock agree to within a return, and every marker the
    /// supervisor places against `record_started_at` lands where it
    /// happened, with no offset for this backend. Move any work after the
    /// origin read and that stops being true.
    ///
    /// The audio sources are started inside that wait, before the origin, so
    /// their first packets are already flowing when tick 0 is taken; the ones
    /// captured before it are dropped by each source's aligner.
    fn start(&mut self, config: RecordConfig) -> Result<(), RecorderError> {
        if self.active.is_some() {
            return Err(RecorderError::AlreadyRecording);
        }
        // Refused before anything comes up: a layout that is not one (a
        // hand-edited `Custom` row) records nothing rather than something.
        // Until #239 the file holds track 0 only, the mix, so that is all the
        // plan opens.
        let layout = select::audio_layout(&config.audio).map_err(RecorderError::Backend)?;
        let plan = plan::plan(&layout, plan::TRACKS_WRITTEN);
        let planned = plan.sources.len();
        if let Some(build) = windows_build()
            && let Some(floor) = select::availability(build)
        {
            // Only reachable through the devtools override (daemon/mod.rs).
            warn!(
                "recorder",
                "own backend: RECORDING BELOW THE OS FLOOR because {} is set: {floor}",
                select::IGNORE_FLOOR_ENV
            );
        }
        std::fs::create_dir_all(&config.output_dir)?;
        let path = config.expected_output_path();

        let (origin_tx, origin_rx) = channel();
        let started = self.ask(|reply| Command::Start {
            path: path.clone(),
            plan,
            reply,
            origin: origin_rx,
        })?;
        let Started { status, audio } = match started {
            Ok(started) => started,
            Err(e) => {
                if self.status == Status::Idle {
                    self.status = Status::Unavailable { reason: e.clone() };
                }
                return Err(RecorderError::Backend(e));
            }
        };
        if let Status::Software { encoder, reason } = &status {
            warn!("recorder", "own backend: software H.264 encoding with {encoder}: {reason}");
        }
        info!(
            "recorder",
            "own backend recording: {}; {}",
            status.backend_name(),
            match audio.tracks.first() {
                Some(track) => format!(
                    "track 0 ({}) mixes {} of the {planned} source(s) planned for it",
                    track.label,
                    track.sources.len()
                ),
                None => "no audio track".to_string(),
            }
        );
        self.status = status;
        self.active = Some(Active { path, audio });

        // The origin, last: see the doc comment above.
        if origin_tx.send(device::qpc_hns()).is_err() {
            self.active = None;
            return Err(RecorderError::Backend(
                "the capture thread exited before recording began".to_string(),
            ));
        }
        Ok(())
    }

    fn stop(&mut self) -> Result<RecordingOutput, RecorderError> {
        let Active { path, audio } = self.active.take().ok_or(RecorderError::NotRecording)?;
        // Bounded, so a finalize that never returns (a driver wedged by a
        // lost GPU, say) cannot hold the supervisor with it.
        let answer = self.ask_within(Some(STOP_WAIT), Command::Stop).and_then(|answer| {
            answer.map_err(RecorderError::Backend)
        });
        match answer {
            Ok(None) => {}
            Ok(Some(problem)) => {
                warn!("recorder", "own backend: the recording ended before stop: {problem}");
            }
            // The file is fragmented, so whatever reached the disk before a
            // failed finalize still plays. Hand it over if there is one.
            Err(e) if path.metadata().is_ok_and(|m| m.len() > 0) => {
                warn!("recorder", "own backend: finalize failed, keeping what was written: {e}");
            }
            Err(e) => return Err(e),
        }

        // The sink writer's fragmented file has no `mfra`, so the review
        // player cannot scrub it until faststart has rewritten the index; the
        // same step, and the same function, as the libobs backend's stop.
        // `audio.tracks.len()` sets track 0's default disposition when there
        // is one. Not while a session thread that never answered may still
        // be writing the file: that one is kept fragmented, playable but not
        // scrubbable, which is the price of not hanging.
        if let Some(ffmpeg_path) = &self.ffmpeg_path
            && self.session.is_some()
            && let Err(e) =
                crate::recorder::remux::remux_faststart(ffmpeg_path, &path, audio.tracks.len())
        {
            warn!("recorder", "faststart remux failed, keeping original (unseekable) file: {e}");
        }

        Ok(RecordingOutput { path, audio })
    }

    fn is_recording(&self) -> bool {
        self.active.is_some()
    }

    /// `own (idle)`, `own (ready: <encoder>)`, `own (software encoding: …)`
    /// or `own (unavailable: …)`. After a start it names the encoder Media
    /// Foundation actually loaded, which is what `RecordingDiagnostics`
    /// records for the file.
    fn backend_name(&self) -> String {
        self.status.backend_name()
    }

    /// The pre-warm (DEVELOPMENT.md §2.2): COM, Media Foundation, the
    /// adapters and encoders, `select::rank`, and the device, all on the
    /// session thread. `start` does the same itself if this never ran.
    fn prepare(&mut self) -> Result<(), RecorderError> {
        if self.active.is_some() {
            return Ok(());
        }
        match self.ask(Command::Prepare)? {
            Ok(status) => {
                if let Status::Software { encoder, reason } = &status {
                    warn!(
                        "recorder",
                        "own backend: will encode in software with {encoder}: {reason}"
                    );
                }
                self.status = status;
                Ok(())
            }
            Err(e) => {
                self.status = Status::Unavailable { reason: e.clone() };
                Err(RecorderError::Backend(e))
            }
        }
    }

    fn release(&mut self) {
        if self.active.is_some() {
            return;
        }
        self.tear_down();
    }
}

impl Drop for OwnRecorder {
    /// The daemon replacing the backend, or shutting down: the session
    /// thread finalizes a recording in flight before it exits, so the file
    /// is complete even though nobody will ask for it.
    fn drop(&mut self) {
        self.tear_down();
    }
}

#[cfg(test)]
mod tests;
