//! Audio sources, each on its own thread. #237 brought the first: the game's
//! own audio by process loopback. #238 adds the microphone and the desktop,
//! from their WASAPI endpoints (`endpoint`), and applications such as Discord,
//! by process loopback on the application's tree.
//!
//! A source thread does nothing but capture. Each packet is converted to
//! stereo f32, stamped by a [`Stamper`] (on QPC if the engine's stamps are
//! real, on the sample count if not; the same decision for every kind of
//! source), and sent to the session thread, which owns the sink writer and
//! the [`crate::recorder::own::mix::Mixdown`] that places every source's
//! packets on the video's clock and mixes them (`track`). One writer thread
//! means no question about the sink writer's own locking.
//!
//! **Every source is asked for the same format**, 48 kHz stereo float, so
//! nothing downstream converts rates: process loopback has no mix format to
//! ask for and converts to whatever it is given, and the endpoints are
//! opened with `AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM`, which puts the engine's
//! own resampler in front of the capture.

mod endpoint;
mod loopback;
mod track;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Sender, SyncSender, sync_channel};
use std::thread::JoinHandle;

use windows::Win32::Media::Audio::{
    AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY, AUDCLNT_BUFFERFLAGS_SILENT,
    AUDCLNT_BUFFERFLAGS_TIMESTAMP_ERROR, IAudioCaptureClient, WAVEFORMATEX,
};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};

use super::device;
use crate::recorder::own::clock::{self, AudioClock, Stamp, Stamper};
use crate::recorder::own::feed::Packet;
use crate::recorder::own::pcm::{self, SampleFormat};
use crate::{info, warn};

pub use track::MixTrack;

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
/// `AUDCLNT_E_DEVICE_INVALIDATED`, which ends its thread.
fn read_packet(capture: &IAudioCaptureClient) -> Result<Option<Raw>, String> {
    // SAFETY: the client is started and live.
    let available = unsafe { capture.GetNextPacketSize() }
        .map_err(|e| format!("GetNextPacketSize failed: {e}"))?;
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
    .map_err(|e| format!("GetBuffer failed: {e}"))?;
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
    unsafe { capture.ReleaseBuffer(frames) }.map_err(|e| format!("ReleaseBuffer failed: {e}"))?;

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

/// A started capture, of whichever kind. Lives on its source thread only.
trait Capture {
    /// Waits up to `ms` for a packet. `false` is a quiet stretch.
    fn wait(&self, ms: u32) -> bool;
    /// The next packet waiting, if any ([`read_packet`]).
    fn next(&self) -> Result<Option<Raw>, String>;
    fn stop(&self);
}

/// What one source thread captures.
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

/// What a source did, reported when it stops.
#[derive(Default)]
pub struct Summary {
    pub clock: Option<AudioClock>,
    pub packets: u64,
    pub silent_packets: u64,
    pub discontinuities: u64,
    /// QPC-mode packets whose own stamp failed the check.
    pub substituted: u64,
    /// Device-mode re-anchors across a hole.
    pub reanchors: u64,
}

/// A running source thread.
pub struct Source {
    stop: Arc<AtomicBool>,
    thread: JoinHandle<Result<Summary, String>>,
}

impl Source {
    /// Stops the capture and waits for the thread. `Err` if it ended on an
    /// error of its own before it was asked to stop (a microphone unplugged,
    /// say), with what it captured up to then lost from the summary but not
    /// from the file.
    pub fn stop(self) -> Result<Summary, String> {
        self.stop.store(true, Ordering::Release);
        self.thread.join().map_err(|_| "the audio thread panicked".to_string())?
    }
}

/// Starts capturing `target` on a thread of its own, named for `name` (`game`,
/// `microphone`, `desktop`, or the application's executable), which is also
/// how the log lines name it. Returns once the capture is running, or with
/// the reason it could not start.
pub fn start(name: &str, target: Target, packets: Sender<Packet>) -> Result<Source, String> {
    let (ready_tx, ready_rx) = sync_channel::<Result<(), String>>(1);
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
            Err(format!("the {name} audio thread exited before it started capturing"))
        }
    }
}

fn thread_main(
    name: &str,
    target: Target,
    packets: Sender<Packet>,
    ready: SyncSender<Result<(), String>>,
    stop: Arc<AtomicBool>,
) -> Result<Summary, String> {
    // MTA: process-loopback activation is asynchronous and completes on an
    // MTA worker, which an STA thread would have to pump for.
    // SAFETY: once, on this thread, before any COM use; paired below.
    if let Err(e) = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.ok() {
        let why = format!("CoInitializeEx (audio thread) failed: {e}");
        let _ = ready.send(Err(why.clone()));
        return Err(why);
    }
    let result = capture(name, target, &packets, &ready, &stop);
    // SAFETY: pairs the CoInitializeEx above; `capture` has dropped every COM
    // object it made by the time it returns.
    unsafe { CoUninitialize() };
    result
}

/// Opens and starts `target`.
fn open(name: &str, target: Target) -> Result<Box<dyn Capture>, String> {
    Ok(match target {
        Target::Process(pid) => {
            let source = loopback::Loopback::open(pid)?;
            source.start()?;
            Box::new(source)
        }
        Target::Microphone(device_id) => {
            let source = endpoint::Endpoint::open(endpoint::Kind::Microphone(device_id))?;
            source.start()?;
            info!("recorder", "own backend: {name} audio from {}", source.describe());
            Box::new(source)
        }
        Target::Desktop => {
            let source = endpoint::Endpoint::open(endpoint::Kind::Desktop)?;
            source.start()?;
            info!("recorder", "own backend: {name} audio from {}", source.describe());
            Box::new(source)
        }
    })
}

fn capture(
    name: &str,
    target: Target,
    packets: &Sender<Packet>,
    ready: &SyncSender<Result<(), String>>,
    stop: &AtomicBool,
) -> Result<Summary, String> {
    let source = match open(name, target) {
        Ok(source) => source,
        Err(e) => {
            let _ = ready.send(Err(e.clone()));
            return Err(e);
        }
    };
    let _ = ready.send(Ok(()));

    let mut stamper = Stamper::new(SAMPLE_RATE);
    let mut summary = Summary::default();
    let mut decided = false;
    let result =
        pump(name, &*source, &mut stamper, &mut summary, &mut decided, packets, stop);
    source.stop();
    summary.clock = stamper.clock();
    summary.substituted = stamper.substituted;
    summary.reanchors = stamper.reanchors();
    result.map(|()| summary)
}

fn pump(
    name: &str,
    source: &dyn Capture,
    stamper: &mut Stamper,
    summary: &mut Summary,
    decided: &mut bool,
    packets: &Sender<Packet>,
    stop: &AtomicBool,
) -> Result<(), String> {
    while !stop.load(Ordering::Acquire) {
        if !source.wait(WAIT_SLICE_MS) {
            continue;
        }
        while let Some(raw) = source.next()? {
            if raw.frames == 0 {
                continue;
            }
            // A stamp the engine itself flags as wrong is no stamp, and does
            // not get to decide the clock.
            let stamp = (!raw.timestamp_error).then_some(raw.qpc);
            let (hns, hole, clock) = stamper.stamp(raw.frames, stamp, raw.arrival);
            if !*decided && stamper.clock().is_some() {
                *decided = true;
                log_first(name, &raw, stamper);
            }
            summary.packets += 1;
            summary.silent_packets += u64::from(raw.silent);
            summary.discontinuities += u64::from(raw.discontinuity);
            let packet = Packet {
                hns,
                frames: raw.frames,
                discontinuity: raw.discontinuity || hole,
                pcm: raw.pcm,
                clock,
            };
            if packets.send(packet).is_err() {
                // The session has gone; nothing will read another packet.
                return Ok(());
            }
        }
    }
    Ok(())
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
             before it was taken ({positions})",
            lag as f64 / 10_000.0
        ),
        Some(Stamp::Zero) => warn!(
            "recorder",
            "own backend: {name} audio clock device: the capture gave no QPC stamp (0), so the \
             audio is stamped from its sample count, anchored at the first packet ({positions})"
        ),
        Some(Stamp::Implausible { lag }) => warn!(
            "recorder",
            "own backend: {name} audio clock device: the QPC stamp is not this process's counter \
             ({:.1} ms from the moment the packet was taken, more than {} ms allows), so the \
             audio is stamped from its sample count, anchored at the first packet ({positions})",
            lag as f64 / 10_000.0,
            clock::STAMP_MAX_LAG / 10_000
        ),
        None => {}
    }
}
