//! Audio sources, each on its own thread. #237 brought the first: the game's
//! own audio by process loopback. #238 adds the microphone and the desktop,
//! from their WASAPI endpoints (`endpoint`), and applications such as Discord,
//! by process loopback on the application's tree.
//!
//! A source thread does nothing but capture. Each packet is converted to
//! stereo f32, stamped by a [`Stamper`] (on QPC if the engine's stamps are
//! real, on the sample count if not; the same decision for every kind of
//! source, except that the desktop's render-loopback stamps may run further
//! ahead of the read, `clock::LOOPBACK_MAX_LEAD`), and sent to the session thread, which owns the encoders and a
//! [`crate::recorder::own::mix::Mixdown`] per written track, placing every
//! source's packets on the video's clock and mixing each track from them
//! (`track`). One thread feeds every encoder and the file, so nothing about
//! them needs a lock.
//!
//! **Every source is asked for the same format**, 48 kHz stereo float, so
//! nothing downstream converts rates: process loopback has no mix format to
//! ask for and converts to whatever it is given, and the endpoints are
//! opened with `AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM`, which puts the engine's
//! own resampler in front of the capture.
//!
//! **A microphone or desktop whose device goes away comes back with it**
//! (#298). Its thread releases the dead stream and opens the device again
//! about once a second until it starts or the recording stops, and a
//! "Windows default" source also moves to a new default when Windows picks
//! one. The decisions are `own::reattach`'s; see [`capture`].

mod endpoint;
mod loopback;
mod track;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Sender, SyncSender, sync_channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows::Win32::Media::Audio::{
    AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY, AUDCLNT_BUFFERFLAGS_SILENT,
    AUDCLNT_BUFFERFLAGS_TIMESTAMP_ERROR, IAudioCaptureClient, WAVEFORMATEX,
};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};

use super::device;
use crate::recorder::own::clock::{self, AudioClock, Stamp, Stamper};
use crate::recorder::own::feed::Packet;
use crate::recorder::own::pcm::{self, SampleFormat};
use crate::recorder::own::problem::SourceError;
use crate::recorder::own::reattach::{self, Outage, Retry};
use crate::{info, warn};

pub use track::AudioTracks;

/// The rate every source is captured at, and the AAC track's: what the mix
/// graph runs at natively, and a rate the AAC encoder takes.
pub const SAMPLE_RATE: u32 = 48_000;
const CHANNELS: u16 = 2;
const BITS: u16 = 32;

/// `WAVE_FORMAT_IEEE_FLOAT`, spelled out rather than pulling in the kernel
/// streaming feature for one documented constant.
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;

/// How long one wait for a packet lasts. Only bounds how late a stop is
/// noticed: a timeout is a quiet stretch, not an error.
const WAIT_SLICE_MS: u32 = 200;

/// 48 kHz stereo 32-bit float: the format every source is initialised with.
fn float_format() -> WAVEFORMATEX {
    let block_align = CHANNELS * BITS / 8;
    WAVEFORMATEX {
        wFormatTag: WAVE_FORMAT_IEEE_FLOAT,
        nChannels: CHANNELS,
        nSamplesPerSec: SAMPLE_RATE,
        nAvgBytesPerSec: SAMPLE_RATE * u32::from(block_align),
        nBlockAlign: block_align,
        wBitsPerSample: BITS,
        cbSize: 0,
    }
}

/// One packet as `GetBuffer` handed it over.
pub struct Raw {
    pub frames: u32,
    pub silent: bool,
    pub discontinuity: bool,
    /// `AUDCLNT_BUFFERFLAGS_TIMESTAMP_ERROR`: the engine says its own stamp
    /// for this packet is wrong, whatever it looks like.
    pub timestamp_error: bool,
    /// `pu64QPCPosition`, as the engine gave it: 100 ns units if it is the
    /// performance counter, and the thing `clock::check_stamp` judges.
    pub qpc: u64,
    /// `pu64DevicePosition`, in frames. Logged for the first packet only,
    /// alongside the QPC stamp, so a box run shows both.
    pub device_position: u64,
    /// The performance counter read just after `GetBuffer` returned.
    pub arrival: i64,
    /// Stereo f32, interleaved; zeros for a silent packet.
    pub pcm: Vec<f32>,
}

/// The next packet waiting on `capture`, or `None` when the engine has none.
/// One wake can cover several packets, so the caller drains until `None`:
/// leaving one behind makes the next `GetBuffer` return it late.
///
/// The same for every kind of source, since each was initialised with
/// [`float_format`]. A removed device fails here, with
/// `AUDCLNT_E_DEVICE_INVALIDATED`, which an endpoint source waits out
/// (`own::reattach`) and which ends any other.
fn read_packet(capture: &IAudioCaptureClient) -> Result<Option<Raw>, Failure> {
    // SAFETY: the client is started and live.
    let available = unsafe { capture.GetNextPacketSize() }
        .map_err(|e| Failure::call("GetNextPacketSize failed", &e))?;
    if available == 0 {
        return Ok(None);
    }
    let mut data: *mut u8 = std::ptr::null_mut();
    let mut frames = 0u32;
    let mut flags = 0u32;
    let mut device_position = 0u64;
    let mut qpc = 0u64;
    // **Both positions are asked for.** The spikes asked for at most one; the
    // QPC one is what puts this source on the video's clock, if it is real,
    // and `clock::Stamper` decides whether it is.
    // SAFETY: every out-parameter is live; the buffer is released below
    // before the next GetBuffer.
    unsafe {
        capture.GetBuffer(
            &mut data,
            &mut frames,
            &mut flags,
            Some(&mut device_position),
            Some(&mut qpc),
        )
    }
    .map_err(|e| Failure::call("GetBuffer failed", &e))?;
    let arrival = device::qpc_hns();

    let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0;
    let pcm = if silent || data.is_null() {
        // A silent packet's buffer contents are undefined, not zero.
        vec![0.0f32; frames as usize * 2]
    } else {
        let bytes = frames as usize * usize::from(CHANNELS) * SampleFormat::F32.bytes();
        // SAFETY: the engine owns `bytes` bytes at `data` until
        // ReleaseBuffer, in the format Initialize accepted.
        let raw = unsafe { std::slice::from_raw_parts(data, bytes) };
        pcm::to_stereo_f32(raw, SampleFormat::F32, CHANNELS, frames)
    };
    // SAFETY: releases exactly the packet GetBuffer handed out.
    unsafe { capture.ReleaseBuffer(frames) }.map_err(|e| Failure::call("ReleaseBuffer failed", &e))?;

    Ok(Some(Raw {
        frames,
        silent,
        discontinuity: flags & AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY.0 as u32 != 0,
        timestamp_error: flags & AUDCLNT_BUFFERFLAGS_TIMESTAMP_ERROR.0 as u32 != 0,
        qpc,
        device_position,
        arrival,
        pcm,
    }))
}

/// A capture call that failed: what it said, and its HRESULT, which is what
/// decides whether the source waits for its device (`reattach::recoverable`).
pub struct Failure {
    pub reason: String,
    pub hresult: u32,
}

impl Failure {
    fn call(what: &str, e: &windows::core::Error) -> Failure {
        Failure { reason: format!("{what}: {e}"), hresult: e.code().0 as u32 }
    }
}

// The pure module spells the codes out; these are the SDK's own.
const _: () = {
    use windows::Win32::Media::Audio::{
        AUDCLNT_E_DEVICE_INVALIDATED, AUDCLNT_E_RESOURCES_INVALIDATED, AUDCLNT_E_SERVICE_NOT_RUNNING,
    };
    assert!(AUDCLNT_E_DEVICE_INVALIDATED.0 as u32 == reattach::DEVICE_INVALIDATED);
    assert!(AUDCLNT_E_SERVICE_NOT_RUNNING.0 as u32 == reattach::SERVICE_NOT_RUNNING);
    assert!(AUDCLNT_E_RESOURCES_INVALIDATED.0 as u32 == reattach::RESOURCES_INVALIDATED);
};

/// A started capture, of whichever kind. Lives on its source thread only.
trait Capture {
    /// Waits up to `ms` for a packet. `false` is a quiet stretch.
    fn wait(&self, ms: u32) -> bool;
    /// The next packet waiting, if any ([`read_packet`]).
    fn next(&self) -> Result<Option<Raw>, Failure>;
    fn stop(&self);
    /// What was opened, for the log: which device and how it was chosen.
    fn describe(&self) -> Option<&str> {
        None
    }
    /// The device's friendly name, for the line that says it came back.
    fn device(&self) -> Option<&str> {
        None
    }
    /// Whether a "Windows default" source's default has moved to another
    /// device since it opened (`reattach::follows_default`). Asked on every
    /// pass; the capture decides how often it really looks.
    fn moved(&self) -> bool {
        false
    }
}

/// What one source thread captures.
#[derive(Clone)]
pub enum Target {
    /// A process tree by process loopback: the game from `root::game_root`,
    /// or an application from `root::application_root`.
    Process(u32),
    /// An input endpoint: `None` is Windows' default communications device,
    /// as the microphone picker and libobs resolve it (`recorder/devices.rs`).
    Microphone(Option<String>),
    /// The default output endpoint, in loopback: everything the machine plays.
    Desktop,
}

impl Target {
    /// Whether this source waits for its device to come back rather than
    /// ending with it: the endpoints do; a process-loopback source follows
    /// its process (`own::reattach`).
    fn reattaches(&self) -> bool {
        matches!(self, Target::Microphone(_) | Target::Desktop)
    }
}

/// What a source did, reported when it stops. Counts every device it
/// captured from, when its device went away and came back.
#[derive(Default)]
pub struct Summary {
    /// The clock of the last device it captured from.
    pub clock: Option<AudioClock>,
    pub packets: u64,
    pub silent_packets: u64,
    pub discontinuities: u64,
    /// QPC-mode packets whose own stamp failed the check.
    pub substituted: u64,
    /// Device-mode re-anchors across a hole.
    pub reanchors: u64,
    /// Each time its device went away (`own::reattach`), in order; the last
    /// one open if it had not come back by the stop.
    pub outages: Vec<Outage>,
    /// Times a "Windows default" source moved to a new default device.
    pub moves: u64,
}

impl Summary {
    /// Adds what one device's stamper decided and counted.
    fn absorb(&mut self, stamper: &Stamper) {
        self.clock = stamper.clock().or(self.clock);
        self.substituted += stamper.substituted;
        self.reanchors += stamper.reanchors();
    }
}

/// A running source thread.
pub struct Source {
    stop: Arc<AtomicBool>,
    thread: JoinHandle<Result<Summary, String>>,
}

impl Source {
    /// Stops the capture and waits for the thread. `Err` if it ended on an
    /// error of its own before it was asked to stop (a process-loopback
    /// source failing, say), with what it captured up to then lost from the
    /// summary but not from the file. An endpoint whose device went away
    /// waits for it instead, so it is `Ok`, with the gap in `outages`.
    pub fn stop(self) -> Result<Summary, String> {
        self.stop.store(true, Ordering::Release);
        self.thread.join().map_err(|_| "the audio thread panicked".to_string())?
    }
}

/// Starts capturing `target` on a thread of its own, named for `name` (`game`,
/// `microphone`, `desktop`, or the application's executable), which is also
/// how the log lines name it. Returns once the capture is running, or with
/// the reason it could not start and the stage it got to (`own::problem`).
pub fn start(name: &str, target: Target, packets: Sender<Packet>) -> Result<Source, SourceError> {
    let (ready_tx, ready_rx) = sync_channel::<Result<(), SourceError>>(1);
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = stop.clone();
    let thread_name = name.to_string();
    let thread = std::thread::Builder::new()
        .name(format!("own-audio-{name}"))
        .spawn(move || thread_main(&thread_name, target, packets, ready_tx, thread_stop))
        .map_err(|e| format!("could not start the {name} audio thread: {e}"))?;
    match ready_rx.recv() {
        Ok(Ok(())) => Ok(Source { stop, thread }),
        Ok(Err(e)) => {
            let _ = thread.join();
            Err(e)
        }
        Err(_) => {
            let _ = thread.join();
            Err(SourceError::open(format!(
                "the {name} audio thread exited before it started capturing"
            )))
        }
    }
}

fn thread_main(
    name: &str,
    target: Target,
    packets: Sender<Packet>,
    ready: SyncSender<Result<(), SourceError>>,
    stop: Arc<AtomicBool>,
) -> Result<Summary, String> {
    // MTA: process-loopback activation is asynchronous and completes on an
    // MTA worker, which an STA thread would have to pump for.
    // SAFETY: once, on this thread, before any COM use; paired below.
    if let Err(e) = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.ok() {
        let why = format!("CoInitializeEx (audio thread) failed: {e}");
        let _ = ready.send(Err(SourceError::open(why.clone())));
        return Err(why);
    }
    let result = capture(name, target, &packets, &ready, &stop);
    // SAFETY: pairs the CoInitializeEx above; `capture` has dropped every COM
    // object it made by the time it returns.
    unsafe { CoUninitialize() };
    result
}

/// Opens and starts `target`. Everything that fails here is an `Open`
/// failure except finding an endpoint device, which `endpoint` reports as a
/// `Find` one (no microphone, no output device).
fn open(target: &Target) -> Result<Box<dyn Capture>, SourceError> {
    Ok(match target {
        Target::Process(pid) => {
            let source = loopback::Loopback::open(*pid)?;
            source.start()?;
            Box::new(source)
        }
        Target::Microphone(device_id) => {
            let source = endpoint::Endpoint::open(endpoint::Kind::Microphone(device_id.clone()))?;
            source.start()?;
            Box::new(source)
        }
        Target::Desktop => {
            let source = endpoint::Endpoint::open(endpoint::Kind::Desktop)?;
            source.start()?;
            Box::new(source)
        }
    })
}

/// One device's capture, and the stamper whose clock its packets are on. A
/// device that replaces another gets a new one: which clock its stamps are
/// on is its own question.
struct Stream {
    capture: Box<dyn Capture>,
    stamper: Stamper,
    /// Whether the clock line has been logged for this device.
    decided: bool,
    /// The first packet from a device that replaced another is a
    /// discontinuity, so the aligner places it by its stamp rather than
    /// reading the gap before it as drift: the same mark
    /// `AudioTracks::restart_game` puts on a restarted game.
    fresh: bool,
}

impl Stream {
    fn new(capture: Box<dyn Capture>, fresh: bool, max_lead: i64) -> Stream {
        Stream { capture, stamper: Stamper::new(SAMPLE_RATE, max_lead), decided: false, fresh }
    }
}

/// How one device's capture ended.
enum Pumped {
    /// Asked to stop, or the session has gone.
    Stopped,
    /// A "Windows default" source's default is another device now.
    Moved,
    /// A call failed: the device went away, or something worse.
    Failed(Failure),
}

/// How often a lost device's thread wakes to check for a stop and whether a
/// retry is due. Only bounds how late either is noticed.
const REOPEN_POLL: Duration = Duration::from_millis(50);

/// Captures `target` until stopped. **An endpoint whose device goes away is
/// waited for** (#298): the failed stream is released, the device is opened
/// again on `reattach::Retry`'s schedule (the configured one by its id,
/// "Windows default" as whatever the default is now), and the reopened
/// device feeds the same channel, so the track sees a gap its mixer has
/// already filled with silence and then the source again. A default source
/// also moves when its default does. Any other failure ends the thread, as
/// it always has.
fn capture(
    name: &str,
    target: Target,
    packets: &Sender<Packet>,
    ready: &SyncSender<Result<(), SourceError>>,
    stop: &AtomicBool,
) -> Result<Summary, String> {
    // Render loopback is stamped with when the device will play a packet,
    // which is after the read; every other source is stamped in the past
    // (`clock::LOOPBACK_MAX_LEAD`).
    let max_lead = match &target {
        Target::Desktop => clock::LOOPBACK_MAX_LEAD,
        Target::Process(_) | Target::Microphone(_) => clock::STAMP_MAX_LEAD,
    };
    let first = match open(&target) {
        Ok(source) => source,
        Err(e) => {
            let why = e.reason.clone();
            let _ = ready.send(Err(e));
            return Err(why);
        }
    };
    if let Some(what) = first.describe() {
        info!("recorder", "own backend: {name} audio from {what}");
    }
    let _ = ready.send(Ok(()));

    let mut summary = Summary::default();
    let mut stream = Stream::new(first, false, max_lead);
    let result = loop {
        match pump(name, &mut stream, &mut summary, packets, stop) {
            Pumped::Stopped => {
                stream.capture.stop();
                summary.absorb(&stream.stamper);
                break Ok(());
            }
            Pumped::Moved => {
                // The new default opens before the old stream stops, so the
                // source is never without one. One that will not open leaves
                // the old running, and is asked again at the next check.
                if let Ok(next) = open(&target) {
                    info!(
                        "recorder",
                        "own backend: {name} audio moved to the new default device: {}",
                        next.describe().unwrap_or("(undescribed)")
                    );
                    stream.capture.stop();
                    summary.absorb(&stream.stamper);
                    summary.moves += 1;
                    stream = Stream::new(next, true, max_lead);
                }
            }
            Pumped::Failed(failure) => {
                stream.capture.stop();
                summary.absorb(&stream.stamper);
                if !target.reattaches() || !reattach::recoverable(Some(failure.hresult)) {
                    break Err(failure.reason);
                }
                let lost = device::qpc_hns();
                warn!(
                    "recorder",
                    "own backend: the {name} audio device went away ({}); it is silence in every \
                     track it feeds until it comes back, which is tried every second",
                    failure.reason
                );
                // Every interface on the lost device is released before the
                // first retry, as Microsoft's recovery describes.
                drop(stream);
                match reopen(name, &target, stop) {
                    Some((next, tries)) => {
                        let back = device::qpc_hns();
                        let outage = Outage { lost, back: Some(back), reason: failure.reason };
                        info!(
                            "recorder",
                            "own backend: {name} came back ({}); captured again after {:.1} s of \
                             silence (try {tries})",
                            next.device().unwrap_or("(unnamed)"),
                            outage.length().unwrap_or_default().as_secs_f64()
                        );
                        if let Some(what) = next.describe() {
                            info!("recorder", "own backend: {name} audio from {what}");
                        }
                        summary.outages.push(outage);
                        stream = Stream::new(next, true, max_lead);
                    }
                    None => {
                        summary.outages.push(Outage { lost, back: None, reason: failure.reason });
                        break Ok(());
                    }
                }
            }
        }
    };
    result.map(|()| summary)
}

/// Opens `target` again on `reattach::Retry`'s schedule until it starts or
/// the source is stopped. Returns the started capture and which try it was.
/// The first failed try is logged, with what it said; the rest are not.
fn reopen(name: &str, target: &Target, stop: &AtomicBool) -> Option<(Box<dyn Capture>, u32)> {
    let began = Instant::now();
    let mut retry = Retry::new();
    while !stop.load(Ordering::Acquire) {
        if retry.due(began.elapsed()) {
            match open(target) {
                Ok(capture) => return Some((capture, retry.attempts + 1)),
                Err(e) => {
                    if retry.attempts == 0 {
                        info!("recorder", "own backend: {name} is not back yet ({e}); still trying");
                    }
                    retry.failed(began.elapsed());
                }
            }
        }
        std::thread::sleep(REOPEN_POLL);
    }
    None
}

/// Captures from one device until it stops, fails, or (for a default source)
/// the default moves.
fn pump(
    name: &str,
    stream: &mut Stream,
    summary: &mut Summary,
    packets: &Sender<Packet>,
    stop: &AtomicBool,
) -> Pumped {
    while !stop.load(Ordering::Acquire) {
        if stream.capture.moved() {
            return Pumped::Moved;
        }
        if !stream.capture.wait(WAIT_SLICE_MS) {
            continue;
        }
        loop {
            let raw = match stream.capture.next() {
                Ok(Some(raw)) => raw,
                Ok(None) => break,
                Err(failure) => return Pumped::Failed(failure),
            };
            if raw.frames == 0 {
                continue;
            }
            // A stamp the engine itself flags as wrong is no stamp, and does
            // not get to decide the clock.
            let stamp = (!raw.timestamp_error).then_some(raw.qpc);
            let substituted = stream.stamper.substituted;
            let (hns, hole, clock) = stream.stamper.stamp(raw.frames, stamp, raw.arrival);
            // On QPC, a substituted stamp is the packet's arrival less its
            // length: an estimate. On device time every stamp is the count.
            let estimated = clock == AudioClock::Qpc && stream.stamper.substituted > substituted;
            if !stream.decided && stream.stamper.clock().is_some() {
                stream.decided = true;
                log_first(name, &raw, &stream.stamper);
            }
            summary.packets += 1;
            summary.silent_packets += u64::from(raw.silent);
            summary.discontinuities += u64::from(raw.discontinuity);
            let packet = Packet {
                hns,
                frames: raw.frames,
                discontinuity: raw.discontinuity || hole || std::mem::take(&mut stream.fresh),
                estimated,
                pcm: raw.pcm,
                clock,
            };
            if packets.send(packet).is_err() {
                // The session has gone; nothing will read another packet.
                return Pumped::Stopped;
            }
        }
    }
    Pumped::Stopped
}

/// The answer to the QPC question, once per source per recording: what the
/// deciding packet's stamps were (the first one the engine did not flag as a
/// timestamp error) and which clock that chose.
fn log_first(name: &str, raw: &Raw, stamper: &Stamper) {
    let positions = format!(
        "QPC position {}, device position {}, taken at {}; {} earlier packet(s) flagged as \
         timestamp errors",
        raw.qpc, raw.device_position, raw.arrival, stamper.substituted
    );
    match stamper.first {
        Some(Stamp::Qpc { lag }) => info!(
            "recorder",
            "own backend: {name} audio clock qpc: the first packet's QPC stamp is real, {:.2} ms \
             {} it was taken ({positions})",
            lag.abs() as f64 / 10_000.0,
            if lag < 0 { "after" } else { "before" }
        ),
        Some(Stamp::Zero) => warn!(
            "recorder",
            "own backend: {name} audio clock device: the capture gave no QPC stamp (0), so the \
             audio is stamped from its sample count, anchored at the first packet ({positions})"
        ),
        Some(Stamp::Implausible { lag }) => warn!(
            "recorder",
            "own backend: {name} audio clock device: the QPC stamp is not this process's counter \
             ({}), so the audio is stamped from its sample count, anchored at the first packet \
             ({positions})",
            clock::rejection(lag, stamper.max_lead())
        ),
        None => {}
    }
}
