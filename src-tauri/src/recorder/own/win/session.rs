//! The session thread: the one thread that holds the own backend's COM
//! objects, and everything it does with them.
//!
//! It runs in the capture worker process (`own::worker`, #241), not the
//! daemon. `host.rs` sends [`Command`]s here over a channel and waits for the
//! reply, translating each from a line on the worker's stdin; the daemon's
//! `OwnRecorder` (in `mod.rs`) is the other end of that pipe and holds none of
//! this. So a fault in anything this thread calls, an encoder MFT's driver
//! above all, ends the worker and not the daemon.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::time::{Duration, Instant};

use windows::Win32::Media::MediaFoundation::{MFSTARTUP_FULL, MFShutdown, MFStartup};
use windows::Win32::Media::{timeBeginPeriod, timeEndPeriod};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};

use super::audio::AudioTracks;
use super::capture::{self, Capture, Slot};
use super::device::{self, Device};
use super::encode;
use super::output::Output;
use super::scale::{self, Fitter};
use crate::recorder::audio::AudioLayout;
use crate::recorder::own::clock;
use crate::recorder::own::fit::Size;
use crate::recorder::own::plan::{self, CapturePlan};
use crate::recorder::own::select::{self, Choice};
use crate::recorder::own::stats;
use crate::recorder::own::status::{self, Status};
use crate::recorder::window;
use crate::{info, warn};

/// The grid every recording is written on (DEVELOPMENT.md §2.4's 60 fps).
pub const FPS: u32 = 60;

/// Textures frames are copied into. Enough that the encoder holding a few
/// never leaves the capture without somewhere to put the next one.
const SLOTS: usize = 8;

/// How long `start` waits for the game window to report a real size and for
/// WGC's first frame, together. `start` runs under the recorder lock on the
/// supervisor's path, so this bounds how long that path can stall; the
/// libobs backend's window wait is about the same.
const START_WAIT: Duration = Duration::from_secs(3);

/// How long the session waits for `start`'s caller to send the origin. The
/// caller does nothing between the reply and the send, so this only fires if
/// it has gone.
const ORIGIN_WAIT: Duration = Duration::from_secs(5);

/// What a start answers with.
pub struct Started {
    /// The status for the encoder that actually loaded.
    pub status: Status,
    /// The audio layout the file holds: the plan's, less every source that
    /// could not open (`plan::realised_layout`). No tracks when none did, in
    /// which case the recording is video only and says so, rather than no
    /// recording at all: the same trade the libobs fork makes ("lose per-app
    /// audio, not all recording").
    pub audio: AudioLayout,
    /// The start line, as logged (`own::stats`).
    pub summary: String,
}

/// What a stop answers with.
pub struct Stopped {
    /// `Ok(None)` for a clean stop, `Ok(Some(_))` for a recording that ended
    /// early but was finalized, and `Err` when the finalize itself failed.
    pub result: Result<Option<String>, String>,
    /// The stop line, as logged (`own::stats`), when a recording was
    /// finalized.
    pub summary: Option<String>,
}

impl Stopped {
    fn not_recording() -> Stopped {
        Stopped { result: Err("not recording".to_string()), summary: None }
    }
}

/// What `OwnRecorder` asks of the session thread. Every variant that expects
/// an answer carries the channel to send it on.
pub enum Command {
    /// The pre-warm: COM, Media Foundation, the adapters and encoders, the
    /// ranking, the device. Answers with the ranked status.
    Prepare(Sender<Result<Status, String>>),
    /// Start recording to `path`, capturing the sources `plan` names. Answers
    /// once the window, the first frame, the audio sources and the encoder
    /// are all up, and then waits on `origin` for the QPC instant that is the
    /// file's t = 0.
    Start {
        path: PathBuf,
        plan: CapturePlan,
        reply: Sender<Result<Started, String>>,
        origin: Receiver<i64>,
    },
    /// Stop and finalize. Answers with how that went ([`Stopped`]).
    Stop(Sender<Stopped>),
    /// Tear down and exit. A recording in flight is finalized first.
    Release,
}

/// Runs the session thread until [`Command::Release`] or until the sender
/// is dropped. Spawned by the worker's `host::SessionHost`; everything it creates is dropped
/// here, on this thread, before COM is uninitialised.
pub fn run(commands: Receiver<Command>) {
    // MTA for the thread's life: WGC's free-threaded pool and Media
    // Foundation both accept it, and this thread has no window to need an
    // STA. WASAPI runs on each audio source's own thread, also MTA.
    // SAFETY: once, on this thread, before any COM use; paired below.
    let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.ok();
    let mut session = Session { warm: None, mf_started: false, ended: None };
    match &com {
        Ok(()) => session.serve(&commands),
        Err(e) => refuse_all(&commands, &format!("CoInitializeEx failed: {e}")),
    }
    session.warm = None;
    if session.mf_started {
        // SAFETY: pairs the successful MFStartup; every MF object is dropped.
        let _ = unsafe { MFShutdown() };
    }
    if com.is_ok() {
        // SAFETY: pairs CoInitializeEx; every COM object has been dropped.
        unsafe { CoUninitialize() };
    }
}

/// Answers every command with `why`, for a thread that could not set up COM.
fn refuse_all(commands: &Receiver<Command>, why: &str) {
    while let Ok(command) = commands.recv() {
        match command {
            Command::Prepare(reply) => {
                let _ = reply.send(Err(why.to_string()));
            }
            Command::Start { reply, .. } => {
                let _ = reply.send(Err(why.to_string()));
            }
            Command::Stop(reply) => {
                let _ = reply.send(Stopped::not_recording());
            }
            Command::Release => return,
        }
    }
}

/// Everything `prepare` brings up, kept until `release`.
struct Warm {
    adapters: Vec<select::Adapter>,
    encoders: Vec<select::Encoder>,
    choice: Choice,
    device: Device,
    /// The adapter the capture runs on, for the start line.
    adapter: String,
}

struct Session {
    warm: Option<Warm>,
    mf_started: bool,
    /// The answer for the next `Stop`, when the recording ended on its own
    /// (a write failed, or the GPU device was lost) and was finalized then.
    ended: Option<Stopped>,
}

/// How a recording's loop ended.
enum Ended {
    /// `stop` asked.
    Stop(Sender<Stopped>),
    /// `release`, or `OwnRecorder` dropped: finalize and exit.
    Exit,
    /// The recording cannot go on (a write failed, or the GPU device was
    /// lost). The game window closing is not one: see [`Recording::run`].
    Problem(String),
}

impl Session {
    fn serve(&mut self, commands: &Receiver<Command>) {
        while let Ok(command) = commands.recv() {
            match command {
                Command::Prepare(reply) => {
                    let _ = reply.send(self.warm_up().map(status_of));
                }
                Command::Start { path, plan, reply, origin } => {
                    self.ended = None;
                    if self.record(path, &plan, reply, origin, commands) {
                        return;
                    }
                }
                Command::Stop(reply) => {
                    let _ = reply.send(self.ended.take().unwrap_or_else(Stopped::not_recording));
                }
                Command::Release => return,
            }
        }
    }

    /// The pre-warm, idempotent. `Err` leaves nothing warm, so the next call
    /// tries again from the start.
    fn warm_up(&mut self) -> Result<&Warm, String> {
        if self.warm.is_none() {
            self.warm = Some(self.bring_up()?);
        }
        Ok(self.warm.as_ref().expect("just set"))
    }

    fn bring_up(&mut self) -> Result<Warm, String> {
        if !self.mf_started {
            // Asked before MFStartup, which is the first delay-loaded call.
            device::media_foundation()?;
            // SAFETY: plain call; paired with MFShutdown when the thread exits.
            unsafe { MFStartup(mf_version(), MFSTARTUP_FULL) }
                .map_err(|e| format!("MFStartup failed: {e}"))?;
            self.mf_started = true;
        }
        let adapters = device::adapters()?;
        let encoders = encode::h264_encoders()?;
        let infos: Vec<select::Adapter> = adapters.iter().map(|a| a.info.clone()).collect();
        let choice = select::rank(&infos, &encoders);
        let adapter = match &choice {
            Choice::Unavailable { reason } => return Err(reason.clone()),
            Choice::Hardware { adapter, .. } => &adapters[*adapter],
            // The software MFT encodes from system memory, so any adapter
            // will do for the capture; a hardware one if there is one.
            Choice::SoftwareFallback { .. } => adapters
                .iter()
                .find(|a| !a.info.software)
                .or(adapters.first())
                .ok_or("DXGI reports no adapter at all")?,
        };
        let device = device::create_device(&adapter.adapter)?;
        info!(
            "recorder",
            "own backend warm: capture on {}; encoder {}; offered: {}",
            adapter.info.name,
            Status::from_choice(&choice, &encoders).backend_name(),
            offered(&encoders)
        );
        let adapter = adapter.info.name.clone();
        Ok(Warm { adapters: infos, encoders, choice, device, adapter })
    }

    /// One recording, from `Start` to the command that ends it. Returns true
    /// if the thread should exit.
    fn record(
        &mut self,
        path: PathBuf,
        plan: &CapturePlan,
        reply: Sender<Result<Started, String>>,
        origin: Receiver<i64>,
        commands: &Receiver<Command>,
    ) -> bool {
        let warm = match self.warm_up() {
            Ok(warm) => warm,
            Err(e) => {
                let _ = reply.send(Err(e));
                return false;
            }
        };
        let mut recording = match Recording::begin(warm, &path, plan) {
            Ok(recording) => recording,
            Err(e) => {
                let _ = reply.send(Err(e));
                return false;
            }
        };
        let started = Started {
            status: recording.status.clone(),
            audio: recording.layout.clone(),
            summary: recording.started.clone(),
        };
        if reply.send(Ok(started)).is_err() {
            // `start`'s caller has gone. Nothing will ever stop this, so do
            // not begin.
            recording.abandon(&path);
            return false;
        }
        let origin = match origin.recv_timeout(ORIGIN_WAIT) {
            Ok(origin) => origin,
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {
                recording.abandon(&path);
                return false;
            }
        };

        // 1 ms scheduler resolution while recording, so the loop's sleeps
        // land within a millisecond of the tick they wait for rather than
        // within 15.6.
        // SAFETY: plain call, paired with timeEndPeriod below.
        unsafe { timeBeginPeriod(1) };
        let ended = recording.run(&self.warm.as_ref().expect("warm").device, origin, commands);
        // SAFETY: pairs timeBeginPeriod(1).
        unsafe { timeEndPeriod(1) };

        let (finalized, summary) = recording.finish();
        let bytes = std::fs::metadata(&path).ok().map(|m| m.len());
        let line = stats::render_stop(&path, FPS, &summary, &finalized, bytes);
        info!("recorder", "{line}");
        let summary = Some(line);
        // A lost device stays lost, and everything warm was made on it: the
        // next `start` warms up again from scratch, on whatever GPU is there.
        if self.warm.as_ref().is_some_and(|warm| device::removed(&warm.device).is_some()) {
            warn!("recorder", "own backend: the GPU device was lost; the next start rebuilds it");
            self.warm = None;
        }
        match ended {
            Ended::Stop(reply) => {
                let _ = reply.send(Stopped { result: finalized.map(|()| None), summary });
                false
            }
            Ended::Exit => {
                if let Err(e) = finalized {
                    warn!("recorder", "own backend: finalizing on exit failed: {e}");
                }
                true
            }
            Ended::Problem(problem) => {
                warn!("recorder", "own backend: the recording ended early: {problem}");
                let result = match finalized {
                    Ok(()) => Ok(Some(problem)),
                    Err(e) => Err(format!("{problem}; then {e}")),
                };
                self.ended = Some(Stopped { result, summary });
                false
            }
        }
    }
}

/// `MF_VERSION`, which windows-rs does not expose as a constant:
/// `(MF_SDK_VERSION << 16) | MF_API_VERSION`, both fixed.
pub fn mf_version() -> u32 {
    const MF_SDK_VERSION: u32 = 0x0002;
    const MF_API_VERSION: u32 = 0x0070;
    (MF_SDK_VERSION << 16) | MF_API_VERSION
}

/// `problem`, or why the device was lost if it was: a failing call on a lost
/// device reports its own symptom, and the removal is the cause.
fn lost_or(device: &Device, problem: String) -> String {
    match device::removed(device) {
        Some(reason) => format!("the GPU device was lost ({reason}): {problem}"),
        None => problem,
    }
}

fn status_of(warm: &Warm) -> Status {
    Status::from_choice(&warm.choice, &warm.encoders)
}

fn offered(encoders: &[select::Encoder]) -> String {
    if encoders.is_empty() {
        return "(no H.264 encoder)".to_string();
    }
    let names: Vec<String> = encoders
        .iter()
        .map(|e| match &e.vendor {
            Some(v) => format!("{} [{v}]", e.name),
            None => format!("{} [{}]", e.name, if e.hardware { "hardware" } else { "software" }),
        })
        .collect();
    names.join("; ")
}

/// One recording in flight.
struct Recording {
    capture: Capture,
    /// The encoders and the file. `None` once finalized.
    output: Option<Output>,
    slots: Vec<Slot>,
    /// The slot holding the frame the next tick shows.
    latest: usize,
    next_slot: usize,
    status: Status,
    /// Puts each frame into a slot, scaling it if the window has changed
    /// size since `start`.
    fitter: Fitter,
    /// WGC said the window closed.
    window_closed: bool,
    /// The slot every tick shows is black now, or has stopped trying to be.
    black: bool,
    /// Every audio track, the mix and the stems, over every source that
    /// opened, or `None` for a video-only recording.
    audio: Option<AudioTracks>,
    /// What the file holds, for `Started`.
    layout: AudioLayout,
    /// Ticks written so far: tick `ticks` is the next one due.
    ticks: u64,
    /// What the stop line sums up (`own::stats`).
    summary: stats::Stop,
    /// The start line, for `Started`.
    started: String,
}

impl Recording {
    /// Finds the window, starts WGC, waits for the first frame, then brings
    /// the encoder up and checks it. Everything that can fail does so before
    /// `start` returns, so a start that succeeds is one that is recording.
    fn begin(warm: &Warm, path: &std::path::Path, plan: &CapturePlan) -> Result<Recording, String> {
        let deadline = Instant::now() + START_WAIT;
        let hwnd = loop {
            if let Some(hwnd) = window::find_by_class()
                && window::client_size(hwnd).is_some()
            {
                break hwnd;
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "no League game window with a real size within {} s (class {})",
                    START_WAIT.as_secs(),
                    window::WINDOW_CLASS
                ));
            }
            std::thread::sleep(Duration::from_millis(50));
        };

        let mut capture = Capture::start(&warm.device, hwnd)?;
        let size = capture.size();
        let (width, height) = status::even_size(size.Width, size.Height)
            .ok_or("the game window has no area to capture (minimised?)")?;
        info!("recorder", "own backend: WGC border {}", capture.border);

        // WGC sends a frame on start for anything visible; a minimised window
        // sends none. At least a second, even if finding the window took most
        // of the budget.
        let frame_deadline = deadline.max(Instant::now() + Duration::from_secs(1));
        let first = loop {
            if let Some(frame) = capture.newest_frame() {
                break frame;
            }
            if Instant::now() >= frame_deadline {
                return Err(
                    "no frame from WGC for the game window: a minimised window produces none, \
                     and exclusive fullscreen may produce none"
                        .to_string(),
                );
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        let slots = capture::create_slots(&warm.device.device, width, height, SLOTS)?;
        let mut fitter = Fitter::new(Size::new(width, height));
        let (texture, content) = capture.texture(&warm.device, &first)?;
        // Black first: a new texture's contents are undefined, and a first
        // frame the fitter skips would otherwise show them.
        scale::fill_black(&warm.device, &slots[0].texture)?;
        fitter.place(
            &warm.device,
            &texture,
            Size::from_i32(content.Width, content.Height),
            &slots[0].texture,
        )?;
        drop(first);

        // Every source the plan names: the game from the process that owns
        // the window being recorded, and the microphone, the desktop or an
        // application beside it. Started before the encoders, because there
        // is an AAC encoder and a track for each track that kept a source; a
        // source that fails costs itself (and any stem it alone fed), not the
        // recording, and the reported layout says which it was.
        let (audio, layout, opened) = AudioTracks::start(hwnd, plan);
        if audio.is_none() && !plan.sources.is_empty() {
            warn!("recorder", "own backend: no audio source opened, recording video only");
        }

        let chosen = match &warm.choice {
            Choice::Hardware { encoder, .. } | Choice::SoftwareFallback { encoder, .. } => {
                warm.encoders.get(*encoder).ok_or("the ranked encoder is not in the list")?
            }
            Choice::Unavailable { reason } => return Err(reason.clone()),
        };
        // Nothing is on disk yet: the file is created at the first keyframe.
        let size = Size::new(width, height);
        let tracks = layout.tracks.len();
        let output = Output::create(path, &warm.device, size, FPS, chosen, SLOTS, tracks)?;
        let (status, warning) =
            status::check_loaded(&warm.choice, &warm.adapters, &warm.encoders, output.loaded())?;
        if let Some(warning) = warning {
            warn!("recorder", "own backend: {warning}");
        }
        info!(
            "recorder",
            "own backend: {width}x{height} at {FPS} fps, H.264 {} Mbps CBR, GOP {}, no B-frames, \
             {}; AAC {} kbps per track, {}; into {} (one fragment per GOP)",
            encode::VIDEO_BITRATE / 1_000_000,
            encode::GOP_FRAMES,
            output.describe(),
            encode::AAC_BYTES_PER_SECOND * 8 / 1000,
            plan::describe(&layout),
            path.display()
        );
        let started =
            stats::render_start(path, (width, height), &warm.adapter, &status, &opened, &layout);
        info!("recorder", "{started}");
        Ok(Recording {
            capture,
            output: Some(output),
            slots,
            latest: 0,
            next_slot: 1,
            status,
            fitter,
            window_closed: false,
            black: false,
            audio,
            layout,
            ticks: 0,
            summary: stats::Stop::default(),
            started,
        })
    }

    /// The cadence loop: take the newest frame WGC has, and write every tick
    /// that is due on the 60 fps grid laid from `origin`. A tick with no new
    /// frame repeats the last one, which is what holds the file at CFR.
    ///
    /// **What the game window can do meanwhile** (#240):
    ///
    /// - *Resize*: WGC's frames change size, the pool is recreated at the new
    ///   one, and the fitter scales each frame into the size the encoder was
    ///   set up for, letterboxed (`fit`, `scale`).
    /// - *Minimise*: WGC delivers nothing at all, so `take_frame` finds no
    ///   frame and every tick repeats the last one until the window comes
    ///   back. Nothing here waits on WGC, so nothing stalls: the file keeps
    ///   its cadence, frozen on the last picture.
    /// - *Close* (the game ended or crashed): WGC raises `Closed`, and the
    ///   loop keeps ticking, black, until `stop`. The supervisor finalizes
    ///   when the Live Client API goes away, about five seconds later; ending
    ///   the recording here instead would leave the file shorter than the
    ///   state machine's idea of it.
    /// - *Lose the GPU* (a driver update, a TDR): the loop ends and what was
    ///   written is finalized; `stop` returns the file and logs why.
    fn run(&mut self, device: &Device, origin: i64, commands: &Receiver<Command>) -> Ended {
        if let Some(audio) = self.audio.as_mut() {
            audio.begin(origin);
        }
        loop {
            match commands.try_recv() {
                Ok(Command::Stop(reply)) => {
                    // Whatever is due up to now goes in before the finalize.
                    if let Err(e) = self.write_due(device, origin) {
                        warn!("recorder", "own backend: the last ticks were not written: {e}");
                    }
                    return Ended::Stop(reply);
                }
                Ok(Command::Release) | Err(TryRecvError::Disconnected) => return Ended::Exit,
                Ok(Command::Prepare(reply)) => {
                    let _ = reply.send(Ok(self.status.clone()));
                }
                Ok(Command::Start { reply, .. }) => {
                    let _ = reply.send(Err("already recording".to_string()));
                }
                Err(TryRecvError::Empty) => {}
            }
            if let Some(reason) = device::removed(device) {
                return Ended::Problem(format!("the GPU device was lost ({reason})"));
            }
            if self.capture.closed() {
                self.go_black(device);
            } else if let Err(e) = self.take_frame(device) {
                return Ended::Problem(lost_or(device, e));
            }
            if let Err(e) = self.write_due(device, origin) {
                return Ended::Problem(lost_or(device, format!("{e} (at tick {})", self.ticks)));
            }
            // A hardware encoder finishes frames between ticks: collect them.
            if let Some(output) = self.output.as_mut()
                && let Err(e) = output.poll()
            {
                return Ended::Problem(lost_or(device, format!("{e} (at tick {})", self.ticks)));
            }
            // Audio keeps flowing after the window closes, under the black
            // frames: a crashed game goes quiet, and the mixer's watermark
            // carries it as silence until `stop`.
            if let Err(e) = self.write_audio(origin) {
                return Ended::Problem(format!("{e} (audio, at tick {})", self.ticks));
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Writes whatever each track's mix has ready, as far as the video has
    /// been written, into that track's encoder: every source that has
    /// delivered, and the rest as silence once the mixer's watermark has
    /// passed (`own::mix`).
    fn write_audio(&mut self, origin: i64) -> Result<(), String> {
        let (Some(output), Some(audio)) = (self.output.as_mut(), self.audio.as_mut()) else {
            return Ok(());
        };
        let video_end = clock::tick_time(self.ticks, FPS);
        let now = device::qpc_hns() - origin;
        audio.write(video_end, now, &mut |track: usize, pcm: &[i16], position: u64| {
            output.write_audio(track, pcm, position)
        })
    }

    /// Puts WGC's newest frame, if there is one, into a free slot. No frame
    /// (a minimised window sends none) leaves the last one showing.
    fn take_frame(&mut self, device: &Device) -> Result<(), String> {
        let Some(frame) = self.capture.newest_frame() else {
            return Ok(());
        };
        let (texture, content) = self.capture.texture(device, &frame)?;
        // No free slot means the encoder is holding every one: the frame is
        // dropped and the tick repeats the last, which is what CFR asks for.
        if let Some(i) = self.free_slot() {
            let content = Size::from_i32(content.Width, content.Height);
            let placed = self.fitter.place(device, &texture, content, &self.slots[i].texture)?;
            if placed != scale::Placed::Skipped {
                self.latest = i;
                self.next_slot = i + 1;
                self.summary.cadence.frame();
            }
        }
        Ok(())
    }

    /// The game window has closed: every tick from here on is black. Logged
    /// once; retried each pass until a slot is free.
    fn go_black(&mut self, device: &Device) {
        if self.black {
            return;
        }
        if !self.window_closed {
            self.window_closed = true;
            info!(
                "recorder",
                "own backend: the game window closed (the game ended or crashed); recording \
                 black until stop"
            );
        }
        let Some(i) = self.free_slot() else {
            return;
        };
        match scale::fill_black(device, &self.slots[i].texture) {
            Ok(()) => {
                self.latest = i;
                self.next_slot = i + 1;
                self.summary.cadence.frame();
            }
            // Not worth ending a recording over: the last frame repeats.
            Err(e) => warn!("recorder", "own backend: could not write black ({e})"),
        }
        self.black = true;
    }

    /// A slot the encoder has let go of and no tick is about to show.
    fn free_slot(&self) -> Option<usize> {
        (0..self.slots.len())
            .map(|o| (self.next_slot + o) % self.slots.len())
            .find(|&i| i != self.latest && !self.slots[i].busy())
    }

    /// Writes every tick from the next one up to the one due now.
    fn write_due(&mut self, device: &Device, origin: i64) -> Result<(), String> {
        let Some(output) = self.output.as_mut() else {
            return Ok(());
        };
        let now = device::qpc_hns() - origin;
        if let Some(due) = clock::ticks_due(now, FPS) {
            while self.ticks <= due {
                let t = clock::tick_time(self.ticks, FPS);
                let d = clock::tick_time(self.ticks + 1, FPS) - t;
                output.write(device, &self.slots, self.latest, t, d)?;
                self.ticks += 1;
                self.summary.cadence.tick(now - t);
            }
        }
        Ok(())
    }

    /// Finalizes the file. The capture stops first; each audio track is
    /// written up to the last video tick and padded to it exactly, as the
    /// spike did, so any A/V offset in the file was added downstream of here;
    /// then every encoder drains into the file and it gets its `mfra`, and
    /// only then do the slots go: every sample made from a slot has to be
    /// released before its callback is, which the encoder's shutdown does.
    ///
    /// With the GPU device lost this still runs: the encoder's drain fails
    /// rather than waits (or gives up after five seconds), and the fragments
    /// already on disk are the recording (`stop` bounds the wait regardless).
    ///
    /// Returns the finalize's result and the counters for the stop line.
    fn finish(mut self) -> (Result<(), String>, stats::Stop) {
        drop(self.capture);
        let end = clock::tick_time(self.ticks, FPS);
        if let Some(audio) = self.audio.take() {
            match self.output.as_mut() {
                Some(output) => {
                    self.summary.audio = audio.finish(end, &mut |track, pcm, position| {
                        output.write_audio(track, pcm, position)
                    });
                }
                None => drop(audio),
            }
        }
        let fragments = &mut self.summary.fragments;
        let result = match self.output.take() {
            Some(output) => output.finalize().map(|stats| {
                *fragments = Some(stats.fragments);
                info!(
                    "recorder",
                    "own backend: file closed: {} video frames ({} keyframes, {} dropped before \
                     the first), AAC frames per track {:?}, {} fragments",
                    stats.video_frames,
                    stats.keyframes,
                    stats.dropped_before_keyframe,
                    stats.audio_frames,
                    stats.fragments
                );
            }),
            None => Ok(()),
        };
        drop(self.slots);
        (result, self.summary)
    }

}

impl Recording {
    /// A recording that never got its origin: nothing was encoded, so there
    /// is normally no file, and one that exists goes too.
    fn abandon(self, path: &std::path::Path) {
        let _ = self.finish();
        let _ = std::fs::remove_file(path);
    }
}
