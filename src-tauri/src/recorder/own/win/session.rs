//! The session thread: the one thread that holds the own backend's COM
//! objects, and everything it does with them.
//!
//! `OwnRecorder` (in `mod.rs`) is `Send` and holds none of this; it sends
//! [`Command`]s here over a channel and waits for the reply. That boundary
//! is deliberate twice over. COM objects on an MTA thread are not something
//! the supervisor's `Mutex<Box<dyn Recorder>>` should be moving between
//! threads, and WS1.6.9 (#241) moves exactly this thread into a capture
//! worker process, where the channel becomes a pipe.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, TryRecvError, channel};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::HWND;
use windows::Win32::Media::MediaFoundation::{MFSTARTUP_FULL, MFShutdown, MFStartup};
use windows::Win32::Media::{timeBeginPeriod, timeEndPeriod};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};

use super::audio;
use super::capture::{self, Capture, Slot};
use super::device::{self, Device};
use super::encode::{self, Sink};
use super::process;
use crate::recorder::own::clock::{self, AudioClock, HNS_PER_SECOND};
use crate::recorder::own::feed::{self, Feed};
use crate::recorder::own::root;
use crate::recorder::own::select::{self, Choice};
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

/// How far behind the video a quiet audio source is held with silence
/// (`feed::Feed::hold`). Far more than a packet's delivery latency, so a
/// packet is never cut for arriving a little late, and short enough that
/// the muxer is never left waiting long on the audio track.
const HOLD_MARGIN: i64 = HNS_PER_SECOND / 2;

/// How long a stop waits for the audio packets of the last video tick,
/// which are still in flight when the stop arrives. Whatever has not come
/// by then is padded with silence.
const TAIL_WAIT: Duration = Duration::from_millis(200);

/// What a start answers with.
pub struct Started {
    /// The status for the encoder that actually loaded.
    pub status: Status,
    /// Whether the file has the game's audio track. `false` when process
    /// loopback could not start, in which case the recording is video only
    /// and says so, rather than no recording at all: the same trade the
    /// libobs fork makes ("lose per-app audio, not all recording").
    pub game_audio: bool,
}

/// What `OwnRecorder` asks of the session thread. Every variant that expects
/// an answer carries the channel to send it on.
pub enum Command {
    /// The pre-warm: COM, Media Foundation, the adapters and encoders, the
    /// ranking, the device. Answers with the ranked status.
    Prepare(Sender<Result<Status, String>>),
    /// Start recording to `path`. Answers once the window, the first frame,
    /// the game audio and the encoder are all up, and then waits on `origin`
    /// for the QPC instant that is the file's t = 0.
    Start { path: PathBuf, reply: Sender<Result<Started, String>>, origin: Receiver<i64> },
    /// Stop and finalize. Answers `Ok(None)` for a clean stop, `Ok(Some(_))`
    /// for a recording that ended early but was finalized, and `Err` when the
    /// finalize itself failed.
    Stop(Sender<Result<Option<String>, String>>),
    /// Tear down and exit. A recording in flight is finalized first.
    Release,
}

/// Runs the session thread until [`Command::Release`] or until the sender
/// is dropped. Spawned by `OwnRecorder`; everything it creates is dropped
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
                let _ = reply.send(Err("not recording".to_string()));
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
}

struct Session {
    warm: Option<Warm>,
    mf_started: bool,
    /// The answer for the next `Stop`, when the recording ended on its own
    /// (the window closed, or a write failed) and was finalized then.
    ended: Option<Result<Option<String>, String>>,
}

/// How a recording's loop ended.
enum Ended {
    /// `stop` asked.
    Stop(Sender<Result<Option<String>, String>>),
    /// `release`, or `OwnRecorder` dropped: finalize and exit.
    Exit,
    /// The recording cannot go on (the window closed, or a write failed).
    Problem(String),
}

impl Session {
    fn serve(&mut self, commands: &Receiver<Command>) {
        while let Ok(command) = commands.recv() {
            match command {
                Command::Prepare(reply) => {
                    let _ = reply.send(self.warm_up().map(status_of));
                }
                Command::Start { path, reply, origin } => {
                    self.ended = None;
                    if self.record(path, reply, origin, commands) {
                        return;
                    }
                }
                Command::Stop(reply) => {
                    let answer =
                        self.ended.take().unwrap_or_else(|| Err("not recording".to_string()));
                    let _ = reply.send(answer);
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
        Ok(Warm { adapters: infos, encoders, choice, device })
    }

    /// One recording, from `Start` to the command that ends it. Returns true
    /// if the thread should exit.
    fn record(
        &mut self,
        path: PathBuf,
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
        let mut recording = match Recording::begin(warm, &path) {
            Ok(recording) => recording,
            Err(e) => {
                let _ = reply.send(Err(e));
                return false;
            }
        };
        let started = Started {
            status: recording.status.clone(),
            game_audio: recording.audio.is_some(),
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

        let finalized = recording.finish();
        match ended {
            Ended::Stop(reply) => {
                let _ = reply.send(finalized.map(|()| None));
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
                self.ended = Some(finalized.map(|()| Some(problem)));
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

/// The game's audio while a recording runs: the source thread, the packets
/// it sends, and the feed that places them once the origin is known.
struct GameAudio {
    /// `None` once stopped. Dropping a `GameAudio` stops it, so every early
    /// return in `Recording::begin` ends the thread.
    source: Option<audio::Source>,
    packets: Receiver<feed::Packet>,
    /// Made when the origin arrives; until then packets wait in the channel.
    feed: Option<Feed>,
    /// Set once the source thread has been seen to end mid-recording, so
    /// that is logged once. The track carries on as held silence.
    ended: bool,
}

impl GameAudio {
    /// Resolves the game's process tree from the window being recorded and
    /// starts process loopback on it.
    fn start(hwnd: HWND) -> Result<GameAudio, String> {
        let procs = process::snapshot()?;
        let root = root::game_root(&procs, process::window_owner(hwnd))?;
        info!("recorder", "own backend: game audio from PID {}, {}", root.pid, root.how);
        let (tx, packets) = channel();
        let source = audio::start_game(root.pid, tx)?;
        Ok(GameAudio { source: Some(source), packets, feed: None, ended: false })
    }

    /// Moves every packet that has arrived into the feed.
    fn receive(&mut self) {
        let Some(feed) = self.feed.as_mut() else {
            return;
        };
        loop {
            match self.packets.try_recv() {
                Ok(packet) => feed.push(packet),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    if !self.ended {
                        self.ended = true;
                        warn!(
                            "recorder",
                            "own backend: the game audio capture stopped before the recording \
                             did; the rest of the track is silence"
                        );
                    }
                    break;
                }
            }
        }
    }

    fn stop_source(&mut self) -> Option<Result<audio::Summary, String>> {
        self.source.take().map(audio::Source::stop)
    }
}

impl Drop for GameAudio {
    fn drop(&mut self) {
        if let Some(Err(e)) = self.stop_source() {
            warn!("recorder", "own backend: the game audio capture ended with an error: {e}");
        }
    }
}

/// One recording in flight.
struct Recording {
    capture: Capture,
    /// `None` once finalized.
    sink: Option<Sink>,
    slots: Vec<Slot>,
    width: u32,
    height: u32,
    /// The slot holding the frame the next tick shows.
    latest: usize,
    next_slot: usize,
    status: Status,
    /// The game's audio, or `None` for a video-only recording.
    audio: Option<GameAudio>,
    /// Ticks written so far: tick `ticks` is the next one due.
    ticks: u64,
}

impl Recording {
    /// Finds the window, starts WGC, waits for the first frame, then brings
    /// the encoder up and checks it. Everything that can fail does so before
    /// `start` returns, so a start that succeeds is one that is recording.
    fn begin(warm: &Warm, path: &std::path::Path) -> Result<Recording, String> {
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
        let (texture, content) = capture.texture(&warm.device, &first)?;
        capture::copy_into(
            &warm.device.context,
            &slots[0],
            &texture,
            width.min(content.Width.max(0) as u32),
            height.min(content.Height.max(0) as u32),
        );
        drop(first);

        // The game's audio, from the process that owns the window being
        // recorded. Started before the sink, because the sink's audio stream
        // exists only if this did; a failure costs the audio track, not the
        // recording, and the reported layout says which it was.
        let audio = match GameAudio::start(hwnd) {
            Ok(audio) => Some(audio),
            Err(e) => {
                warn!("recorder", "own backend: no game audio, recording video only: {e}");
                None
            }
        };

        let hardware = matches!(warm.choice, Choice::Hardware { .. });
        let audio_rate = audio.as_ref().map(|_| audio::SAMPLE_RATE);
        let sink =
            Sink::create(path, width, height, FPS, &warm.device.device, hardware, audio_rate)
                .and_then(|sink| sink.begin().map(|()| sink));
        let sink = match sink {
            Ok(sink) => sink,
            Err(e) => {
                let _ = std::fs::remove_file(path);
                return Err(e);
            }
        };
        let checked = sink.loaded().and_then(|loaded| {
            status::check_loaded(&warm.choice, &warm.adapters, &warm.encoders, &loaded)
        });
        let (status, warning) = match checked {
            Ok(checked) => checked,
            Err(e) => {
                let _ = sink.finalize();
                let _ = std::fs::remove_file(path);
                return Err(e);
            }
        };
        if let Some(warning) = warning {
            warn!("recorder", "own backend: {warning}");
        }
        info!(
            "recorder",
            "own backend: {width}x{height} at {FPS} fps, H.264 {} Mbps CBR, GOP {}, {}, into {}",
            encode::VIDEO_BITRATE / 1_000_000,
            encode::GOP_FRAMES,
            if sink.has_audio() {
                format!(
                    "AAC {} kbps (game, process loopback)",
                    encode::AAC_BYTES_PER_SECOND * 8 / 1000
                )
            } else {
                "no audio".to_string()
            },
            path.display()
        );
        Ok(Recording {
            capture,
            sink: Some(sink),
            slots,
            width,
            height,
            latest: 0,
            next_slot: 1,
            status,
            audio,
            ticks: 0,
        })
    }

    /// The cadence loop: take the newest frame WGC has, and write every tick
    /// that is due on the 60 fps grid laid from `origin`. A tick with no new
    /// frame repeats the last one, which is what holds the file at CFR.
    fn run(&mut self, device: &Device, origin: i64, commands: &Receiver<Command>) -> Ended {
        if let Some(audio) = self.audio.as_mut() {
            audio.feed = Some(Feed::new(audio::SAMPLE_RATE, origin));
        }
        loop {
            match commands.try_recv() {
                Ok(Command::Stop(reply)) => {
                    // Whatever is due up to now goes in before the finalize.
                    if let Err(e) = self.write_due(origin) {
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
            if self.capture.closed() {
                return Ended::Problem("the game window closed".to_string());
            }
            if let Err(e) = self.take_frame(device) {
                return Ended::Problem(e);
            }
            if let Err(e) = self.write_due(origin) {
                return Ended::Problem(format!("{e} (at tick {})", self.ticks));
            }
            if let Err(e) = self.write_audio() {
                return Ended::Problem(format!("{e} (audio, at tick {})", self.ticks));
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Writes the game audio that has arrived, as far as the video has been
    /// written, and holds a quiet source with silence a margin behind it.
    fn write_audio(&mut self) -> Result<(), String> {
        let (Some(sink), Some(audio)) = (&self.sink, self.audio.as_mut()) else {
            return Ok(());
        };
        audio.receive();
        let Some(feed) = audio.feed.as_mut() else {
            return Ok(());
        };
        let video_end = clock::tick_time(self.ticks, FPS);
        let mut write = |pcm: &[i16], position: u64| sink.write_audio(pcm, position);
        feed.write_ready(video_end, &mut write)?;
        feed.hold(video_end, HOLD_MARGIN, &mut write)?;
        Ok(())
    }

    /// Copies WGC's newest frame, if there is one, into a free slot.
    fn take_frame(&mut self, device: &Device) -> Result<(), String> {
        let Some(frame) = self.capture.newest_frame() else {
            return Ok(());
        };
        let (texture, content) = self.capture.texture(device, &frame)?;
        let free = (0..self.slots.len())
            .map(|o| (self.next_slot + o) % self.slots.len())
            .find(|&i| i != self.latest && !self.slots[i].busy());
        // No free slot means the encoder is holding every one: the frame is
        // dropped and the tick repeats the last, which is what CFR asks for.
        if let Some(i) = free {
            let w = self.width.min(content.Width.max(0) as u32);
            let h = self.height.min(content.Height.max(0) as u32);
            capture::copy_into(&device.context, &self.slots[i], &texture, w, h);
            self.latest = i;
            self.next_slot = i + 1;
        }
        Ok(())
    }

    /// Writes every tick from the next one up to the one due now.
    fn write_due(&mut self, origin: i64) -> Result<(), String> {
        let Some(sink) = &self.sink else {
            return Ok(());
        };
        let now = device::qpc_hns() - origin;
        if let Some(due) = clock::ticks_due(now, FPS) {
            while self.ticks <= due {
                let t = clock::tick_time(self.ticks, FPS);
                let d = clock::tick_time(self.ticks + 1, FPS) - t;
                sink.write(&self.slots[self.latest], t, d)?;
                self.ticks += 1;
            }
        }
        Ok(())
    }

    /// Finalizes the file. The capture stops first; the audio is written up
    /// to the last video tick and padded to it exactly, as the spike did, so
    /// any A/V offset in the file was added downstream of here; then the sink
    /// drains, and only then do the slots go: every sample written from a
    /// slot has to be released before its callback is.
    fn finish(mut self) -> Result<(), String> {
        drop(self.capture);
        let end = clock::tick_time(self.ticks, FPS);
        if let Some(audio) = self.audio.take() {
            finish_audio(self.sink.as_ref(), audio, end);
        }
        let result = self.sink.take().map_or(Ok(()), Sink::finalize);
        drop(self.slots);
        result
    }

}

/// Ends the game audio track at `end`, the end of the last video tick: waits
/// briefly for the packets still in flight, stops the source, writes what
/// came, pads to `end`, and logs what the clocks did. Errors are logged
/// rather than returned, because the file is finalized either way.
fn finish_audio(sink: Option<&Sink>, mut audio: GameAudio, end: i64) {
    if audio.feed.is_some() {
        let deadline = Instant::now() + TAIL_WAIT;
        loop {
            audio.receive();
            let reached = audio.feed.as_ref().is_some_and(|f| f.reaches(end));
            if reached || audio.ended || Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    let summary = audio.stop_source();
    // Stopped on purpose: the channel closing now is not the source dying.
    audio.ended = true;
    audio.receive();

    let (Some(sink), Some(feed)) = (sink, audio.feed.as_mut()) else {
        return;
    };
    let mut write = |pcm: &[i16], position: u64| sink.write_audio(pcm, position);
    let padded = match feed.finish(end, &mut write) {
        Ok((pad, _)) => pad,
        Err(e) => {
            warn!("recorder", "own backend: the end of the game audio was not written: {e}");
            0
        }
    };

    let a = feed.aligner();
    let s = &a.stats;
    let rate = a.rate();
    let ms = |samples: i64| clock::samples_to_ms(samples, rate);
    let clock_line = match a.clock() {
        AudioClock::Qpc => format!(
            "clock qpc: raw drift {} ({:.2} ms at the end, worst {:.2} ms); written residual worst \
             {:.2} ms; slips {} dropped, {} repeated",
            a.raw_ppm().map_or_else(|| "not measured".to_string(), |ppm| format!("{ppm:+.2} ppm")),
            ms(s.raw_drift_last),
            ms(s.raw_drift_worst),
            ms(s.residual_worst),
            s.slips_dropped,
            s.slips_repeated
        ),
        AudioClock::Device => {
            "clock device: stamped from the sample count, so raw drift is not measured".to_string()
        }
    };
    let source_line = match summary {
        Some(Ok(sum)) => format!(
            "{} packets, {} flagged silent, {} discontinuities, {} stamps substituted, {} \
             re-anchors",
            sum.packets, sum.silent_packets, sum.discontinuities, sum.substituted, sum.reanchors
        ),
        Some(Err(e)) => format!("the capture ended with an error: {e}"),
        None => "the capture had already stopped".to_string(),
    };
    info!(
        "recorder",
        "own backend: game audio {clock_line}; gaps {} ({:.1} ms), overlaps {} ({:.1} ms), held \
         {} times ({:.1} ms); lead silence {:.1} ms, lead dropped {:.1} ms; padded {:.1} ms at the \
         end; {:.3} s written; {source_line}",
        s.gaps,
        ms(s.gap_samples as i64),
        s.overlaps,
        ms(s.overlap_samples as i64),
        s.holds,
        ms(s.held_samples as i64),
        ms(s.lead_silence as i64),
        ms(s.lead_dropped as i64),
        ms(padded as i64),
        a.written() as f64 / f64::from(rate)
    );
}

impl Recording {
    /// A recording that never got its origin: nothing was written but the
    /// header, so the file goes too.
    fn abandon(self, path: &std::path::Path) {
        let _ = self.finish();
        let _ = std::fs::remove_file(path);
    }
}
