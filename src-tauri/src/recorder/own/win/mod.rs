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
//!   their endpoints), and every written track, the mix and the stems.
//! - `encode` — the encoder MFTs on offer, and what driving one directly
//!   takes: activation, media types, samples in and out.
//! - `h264` — the H.264 encoder MFT, asynchronous (hardware) or synchronous
//!   (software), with textures or system memory in.
//! - `aac` — one AAC encoder MFT per written audio track.
//! - `convert` — each tick's slot as NV12 for the encoder: the video
//!   processor, or the CPU where there is none.
//! - `output` — the frames, the encoders and the file together, which is
//!   what replaced the sink writer (#239).
//! - `session` — the thread that owns all of the above.
//! - `host` — the session thread as the capture worker's [`Host`], driven
//!   by the worker's pipe loop.
//!
//! [`OwnRecorder`] is the `Recorder` the daemon builds. It owns no COM object
//! and no capture thread: it is a client of the capture worker
//! (`ninja-recorder.exe --capture-worker`, `own::worker`), and the session
//! thread runs in that process.
//!
//! [`Host`]: crate::recorder::own::worker::serve::Host

mod aac;
mod audio;
mod capture;
mod convert;
mod device;
mod encode;
mod h264;
pub(crate) mod host;
mod output;
mod process;
mod scale;
mod session;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::{plan, select, stats};
use super::status::Status;
use super::worker::client::Worker;
use super::worker::lifetime::{Action, Call, Lifetime};
use super::worker::protocol::{Reply, Request, Started};
use crate::recorder::audio::AudioLayout;
use crate::recorder::{RecordConfig, Recorder, RecorderError, RecordingOutput};
use crate::{info, warn};

pub use device::windows_build;

/// How long the worker has to answer `Prepare`: COM, Media Foundation, the
/// adapters and the encoder list, and a device. Seconds on a slow machine.
const PREPARE_WAIT: Duration = Duration::from_secs(30);

/// How long the worker has to answer `Start`: the session's own three-second
/// wait for the window and its first frame, plus the whole pre-warm if
/// `prepare` never ran.
const START_WAIT: Duration = Duration::from_secs(30);

/// How long the worker has to finalize a recording. A clean finalize drains a
/// few frames and writes one fragment, well under a second; this is for one
/// that never returns (a driver wedged by a lost GPU, say). The worker is then
/// killed, so nothing is still writing the file, and it is kept and remuxed
/// like any other the worker died on.
const STOP_WAIT: Duration = Duration::from_secs(20);

/// How long a released worker has to exit, finalizing anything in flight on
/// the way, before it is killed.
const SHUTDOWN_WAIT: Duration = Duration::from_secs(30);

/// The own capture backend (Option B): WGC → D3D11 → Media Foundation, in
/// the capture worker.
///
/// **`Send`, and holding no COM object and no capture thread.** Every D3D,
/// Media Foundation and WinRT object lives on the session thread
/// (`session.rs`), in the worker process (`own::worker`), which this drives
/// over the worker's stdin and stdout and waits on for each answer. So a
/// driver fault inside an encoder takes down the worker, not the daemon.
///
/// The worker exists only while League does (`worker::lifetime`): `prepare`
/// spawns it, `release` ends it once nothing is recording, and a worker that
/// dies mid-recording leaves a file that `stop` hands over anyway.
///
/// Every audio preset since #238, and every track of it since #239: the mix
/// and each stem, in one file. Reachable only from a devtools build that selects it
/// (DEVELOPMENT.md §16, "The switch, and when it applies").
pub struct OwnRecorder {
    /// `ninja-recorder.exe` itself, or `None` if the process cannot name its
    /// own executable, in which case nothing can be recorded and `start`
    /// says so.
    exe: Option<PathBuf>,
    worker: Option<Worker>,
    /// The client closed during a recording: end the worker after the stop.
    release_pending: bool,
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

impl OwnRecorder {
    /// Cheap and infallible, like `LibObsRecorder::new`: nothing comes up,
    /// and no worker is spawned, until `prepare` or `start`.
    pub fn new(ffmpeg_path: Option<PathBuf>) -> Self {
        Self::with_worker(std::env::current_exe().ok(), ffmpeg_path)
    }

    /// With the worker executable named, for a test whose own executable is
    /// the test harness rather than `ninja-recorder.exe`.
    pub fn with_worker(exe: Option<PathBuf>, ffmpeg_path: Option<PathBuf>) -> Self {
        Self {
            exe,
            worker: None,
            release_pending: false,
            status: Status::Idle,
            active: None,
            ffmpeg_path,
        }
    }

    /// Asks [`Lifetime::decide`] what `call` means for the worker, after
    /// noticing a worker that has died since the last call.
    fn decide(&mut self, call: Call) -> Action {
        if let Some(why) = self.worker.as_mut().and_then(Worker::exited) {
            warn!("recorder", "own backend: {why} since it was last asked anything");
            self.worker = None;
        }
        let mut lifetime = Lifetime {
            worker: self.worker.is_some(),
            recording: self.active.is_some(),
            release_pending: self.release_pending,
        };
        let action = lifetime.decide(call);
        self.release_pending = lifetime.release_pending;
        action
    }

    /// Spawns a worker for `action` if it asks for one.
    fn spawn_for(&mut self, action: Action) -> Result<(), RecorderError> {
        if action != Action::SpawnAndSend {
            return Ok(());
        }
        let exe = self.exe.as_deref().ok_or_else(|| {
            RecorderError::Backend("cannot find this executable to start the capture worker".into())
        })?;
        match Worker::spawn(exe, crate::log::dir().as_deref()) {
            Ok(worker) => {
                info!("recorder", "own backend: capture worker up, pid {}", worker.pid());
                self.worker = Some(worker);
                Ok(())
            }
            Err(e) => {
                warn!("recorder", "own backend: {e}");
                self.status = Status::Unavailable { reason: e.clone() };
                Err(RecorderError::Backend(e))
            }
        }
    }

    /// Sends `request` to the worker and waits for the answer. On any failure
    /// the worker is gone: it is logged, with its exit code, and dropped, and
    /// the next `prepare` or `start` spawns a fresh one.
    fn ask(&mut self, request: Request, timeout: Duration) -> Result<Reply, RecorderError> {
        let Some(worker) = self.worker.as_mut() else {
            return Err(RecorderError::Backend("the capture worker is not running".into()));
        };
        worker.ask(&request, timeout).map_err(|e| {
            warn!("recorder", "own backend: {e}");
            self.worker = None;
            RecorderError::Backend(e)
        })
    }

    /// A reply that does not answer the request: the worker is out of step,
    /// so it is ended rather than trusted with the next one.
    fn out_of_step(&mut self, reply: Reply) -> RecorderError {
        let why = format!("the capture worker answered out of step: {reply:?}");
        warn!("recorder", "own backend: {why}");
        self.shut_down();
        RecorderError::Backend(why)
    }

    /// Ends the worker, finalizing anything it is still recording, and waits
    /// for it.
    fn shut_down(&mut self) {
        if let Some(worker) = self.worker.take() {
            info!("recorder", "own backend: {}", worker.shut_down(SHUTDOWN_WAIT));
        }
    }
}

impl Recorder for OwnRecorder {
    /// Starts recording the game window into `config`'s `.mp4`, spawning the
    /// capture worker first if it is not up.
    ///
    /// **Video time zero is the last thing this does.** The worker's session
    /// finds the window, waits (about 3 s at most) for a real size and WGC's
    /// first frame, and brings the encoder up; only then does this read the
    /// performance counter and send that instant over as the origin of the
    /// tick grid. QPC is one clock for the whole machine, so the instant read
    /// here is the instant the worker places tick 0 at. It is taken
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
        // Every track is written, so the plan opens every source it names.
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

        let action = self.decide(Call::Start);
        self.spawn_for(action)?;
        // The plan crosses the pipe as the layout it describes, which is
        // `CapturePlan::layout`'s exact inverse.
        let request = Request::Start { path: path.clone(), plan: plan.layout() };
        let started = match self.ask(request, START_WAIT)? {
            Reply::Started { result } => result,
            other => return Err(self.out_of_step(other)),
        };
        let Started { status, audio, summary } = match started {
            Ok(started) => started,
            Err(e) => {
                if self.status == Status::Idle {
                    self.status = Status::Unavailable { reason: e.clone() };
                }
                return Err(RecorderError::Backend(e));
            }
        };
        // The worker logged it to `worker.log`; this is the copy.
        if let Some(summary) = summary {
            info!("recorder", "{summary}");
        }
        if let Status::Software { encoder, reason } = &status {
            warn!("recorder", "own backend: software H.264 encoding with {encoder}: {reason}");
        }
        info!(
            "recorder",
            "own backend recording: {}; {} of the {planned} source(s) planned opened; {}",
            status.backend_name(),
            audio.sources.len(),
            plan::describe(&audio)
        );
        self.status = status;

        // The origin, last: see the doc comment above.
        let origin = Request::Origin { qpc_hns: device::qpc_hns() };
        let sent = self.worker.as_mut().map(|worker| worker.tell(&origin));
        if let Some(Err(e)) = sent {
            warn!("recorder", "own backend: {e}");
            self.worker = None;
            return Err(RecorderError::Backend(e));
        }
        self.active = Some(Active { path, audio });
        Ok(())
    }

    /// Finalizes the recording in the worker, and remuxes it.
    ///
    /// **A worker that has died still hands over its file.** It is
    /// fragmented, so whatever reached the disk before the worker went is
    /// playable up to the last complete fragment; the death is logged with
    /// the worker's exit code, `mp4::write::repair` cuts any torn tail and
    /// writes the `mfra` the worker never did (#239), the file gets the same
    /// faststart remux as a clean stop, and this returns it rather than an
    /// error. Only a worker that died before writing anything at all is an
    /// error.
    fn stop(&mut self) -> Result<RecordingOutput, RecorderError> {
        let action = self.decide(Call::Stop);
        let Active { path, audio } = self.active.take().ok_or(RecorderError::NotRecording)?;
        // Carried out below if the worker is still there to end; a pending
        // release means nothing once nothing is recording.
        self.release_pending = false;
        let answer = match action {
            Action::Send | Action::SendThenShutDown => match self.ask(Request::Stop, STOP_WAIT) {
                Ok(Reply::Stopped { result, summary }) => {
                    // The worker logged it to `worker.log`; this is the copy.
                    if let Some(summary) = summary {
                        info!("recorder", "{summary}");
                    }
                    Some(result)
                }
                Ok(other) => {
                    let _ = self.out_of_step(other);
                    None
                }
                // Logged, with the exit code, by `ask`.
                Err(_) => None,
            },
            // Died since the last call; `decide` logged how.
            _ => None,
        };
        if action == Action::SendThenShutDown {
            // The client closed while this was recording (`release`).
            self.shut_down();
            self.status = Status::Idle;
        }
        let has_bytes = path.metadata().is_ok_and(|m| m.len() > 0);
        // The worker closed the file itself, `mfra` and all.
        let finalized = matches!(answer, Some(Ok(_)));
        match answer {
            Some(Ok(None)) => {}
            Some(Ok(Some(problem))) => {
                warn!("recorder", "own backend: the recording ended before stop: {problem}");
            }
            // The file is fragmented, so whatever reached the disk before a
            // failed finalize still plays. Hand it over if there is one.
            Some(Err(e)) if has_bytes => {
                warn!("recorder", "own backend: finalize failed, keeping what was written: {e}");
            }
            Some(Err(e)) => return Err(RecorderError::Backend(e)),
            None if has_bytes => {
                warn!(
                    "recorder",
                    "own backend: the capture worker died mid-recording; keeping what reached \
                     the disk, which plays up to its last fragment: {}",
                    path.display()
                );
            }
            None => {
                return Err(RecorderError::Backend(
                    "the capture worker died before writing anything".to_string(),
                ));
            }
        }

        // Not closed by the worker: finish it here, in Rust, as startup
        // recovery does. `repair` cuts a fragment the worker died inside and
        // appends the `mfra`, and refuses anything it did not write.
        let repair = (!finalized).then(|| {
            let repaired = crate::mp4::write::repair(&path).map_err(|e| e.to_string());
            if let Err(e) = &repaired {
                warn!("recorder", "own backend: could not repair {}: {e}", path.display());
            }
            repaired
        });

        // The file is fragmented with an `mfra` at the end, and whether the
        // review player (WebView2) seeks such a file has not been verified, so
        // it gets the faststart remux that moves the index to the front; the
        // same step, and the same function, as the libobs backend's stop and
        // startup recovery. `audio.tracks.len()` is every track the file
        // holds, and sets track 0's default disposition when there is one.
        let remux = self.ffmpeg_path.as_ref().map(|ffmpeg| {
            let began = Instant::now();
            let result = crate::recorder::remux::remux_faststart(ffmpeg, &path, audio.tracks.len());
            (result, began.elapsed())
        });
        if let Some((Err(e), _)) = &remux {
            warn!("recorder", "faststart remux failed, keeping original (unseekable) file: {e}");
        }
        info!("recorder", "{}", stats::render_remux(&path, repair.as_ref(), remux.as_ref()));

        Ok(RecordingOutput { path, audio })
    }

    fn is_recording(&self) -> bool {
        self.active.is_some()
    }

    /// `own (idle)`, `own (ready: <encoder>)`, `own (software encoding: …)`
    /// or `own (unavailable: …)`. After a start it names the encoder Media
    /// Foundation actually loaded, which is what `RecordingDiagnostics`
    /// records for the file. A worker dying mid-recording leaves it as it
    /// was, because it still describes the file that was written.
    fn backend_name(&self) -> String {
        self.status.backend_name()
    }

    fn current_file(&self) -> Option<PathBuf> {
        self.active.as_ref().map(|active| active.path.clone())
    }

    /// Whether a worker has been spawned and not yet seen to end. One that
    /// died since the last call still counts until `decide` notices it.
    fn worker_running(&self) -> Option<bool> {
        Some(self.worker.is_some())
    }

    /// The pre-warm (DEVELOPMENT.md §2.2): spawns the capture worker, and in
    /// it COM, Media Foundation, the adapters and encoders, `select::rank`,
    /// and the device. `start` does the same itself if this never ran.
    fn prepare(&mut self) -> Result<(), RecorderError> {
        let action = self.decide(Call::Prepare);
        if action == Action::Nothing {
            return Ok(());
        }
        self.spawn_for(action)?;
        let prepared = match self.ask(Request::Prepare, PREPARE_WAIT)? {
            Reply::Prepared { result } => result,
            other => return Err(self.out_of_step(other)),
        };
        match prepared {
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

    /// Ends the worker: the League client has closed. While recording, the
    /// worker is kept until `stop` has finalized, and ended then.
    fn release(&mut self) {
        if self.decide(Call::Release) == Action::ShutDown {
            self.shut_down();
        }
        if self.active.is_none() {
            self.status = Status::Idle;
        }
    }

    // `collect_output` stays the default no-op: the worker writes its own
    // `worker.log` as it goes, and its stderr is drained into `daemon.log` by
    // a thread (`worker::client`), so nothing waits in a pipe for a command.
}

impl Drop for OwnRecorder {
    /// The daemon replacing the backend, or shutting down: the worker
    /// finalizes a recording in flight before it exits, so the file is
    /// complete even though nobody will ask for it.
    fn drop(&mut self) {
        self.shut_down();
    }
}

#[cfg(test)]
mod tests;
