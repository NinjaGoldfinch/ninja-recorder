//! Audio sources, each on its own thread. #237 brings the first: the game's
//! own audio by process loopback. #238 adds the microphone, the desktop and
//! applications beside it.
//!
//! A source thread does nothing but capture. Each packet is converted to
//! stereo i16, stamped by a [`Stamper`] (on QPC if the engine's stamps are
//! real, on the sample count if not), and sent to the session thread, which
//! owns the sink writer and the [`crate::recorder::own::feed::Feed`] that
//! places the packets on the video's clock. One writer thread means no
//! question about the sink writer's own locking.

mod loopback;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Sender, SyncSender, sync_channel};
use std::thread::JoinHandle;

use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};

use crate::recorder::own::clock::{self, AudioClock, Stamp, Stamper};
use crate::recorder::own::feed::Packet;
use crate::{info, warn};

pub use loopback::SAMPLE_RATE;

/// How long one wait on the capture event lasts. Only bounds how late a
/// stop is noticed: a timeout is a quiet stretch, not an error.
const WAIT_SLICE_MS: u32 = 200;

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
    /// error of its own before it was asked to stop, with what it captured
    /// up to then lost from the summary but not from the file.
    pub fn stop(self) -> Result<Summary, String> {
        self.stop.store(true, Ordering::Release);
        self.thread.join().map_err(|_| "the audio thread panicked".to_string())?
    }
}

/// Starts capturing the process tree rooted at `pid` (the game, from
/// `root::game_root`). Returns once the capture is running, or with the
/// reason it could not start: the activation, the format or the start.
pub fn start_game(pid: u32, packets: Sender<Packet>) -> Result<Source, String> {
    let (ready_tx, ready_rx) = sync_channel::<Result<(), String>>(1);
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = stop.clone();
    let thread = std::thread::Builder::new()
        .name("own-audio-game".to_string())
        .spawn(move || thread_main(pid, packets, ready_tx, thread_stop))
        .map_err(|e| format!("could not start the game audio thread: {e}"))?;
    match ready_rx.recv() {
        Ok(Ok(())) => Ok(Source { stop, thread }),
        Ok(Err(e)) => {
            let _ = thread.join();
            Err(e)
        }
        Err(_) => {
            let _ = thread.join();
            Err("the game audio thread exited before it started capturing".to_string())
        }
    }
}

fn thread_main(
    pid: u32,
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
    let result = capture(pid, &packets, &ready, &stop);
    // SAFETY: pairs the CoInitializeEx above; `capture` has dropped every COM
    // object it made by the time it returns.
    unsafe { CoUninitialize() };
    result
}

fn capture(
    pid: u32,
    packets: &Sender<Packet>,
    ready: &SyncSender<Result<(), String>>,
    stop: &AtomicBool,
) -> Result<Summary, String> {
    let source = match loopback::Loopback::open(pid).and_then(|l| l.start().map(|()| l)) {
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
    let result = pump(&source, &mut stamper, &mut summary, &mut decided, packets, stop);
    source.stop();
    summary.clock = stamper.clock();
    summary.substituted = stamper.substituted;
    summary.reanchors = stamper.reanchors();
    result.map(|()| summary)
}

fn pump(
    source: &loopback::Loopback,
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
                log_first(&raw, stamper);
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

/// The answer to the QPC question, once per recording: what the deciding
/// packet's stamps were (the first one the engine did not flag as a
/// timestamp error) and which clock that chose.
fn log_first(raw: &loopback::Raw, stamper: &Stamper) {
    let positions = format!(
        "QPC position {}, device position {}, taken at {}; {} earlier packet(s) flagged as \
         timestamp errors",
        raw.qpc, raw.device_position, raw.arrival, stamper.substituted
    );
    match stamper.first {
        Some(Stamp::Qpc { lag }) => info!(
            "recorder",
            "own backend: game audio clock qpc: the first packet's QPC stamp is real, {:.2} ms \
             before it was taken ({positions})",
            lag as f64 / 10_000.0
        ),
        Some(Stamp::Zero) => warn!(
            "recorder",
            "own backend: game audio clock device: process loopback gave no QPC stamp (0), so \
             the audio is stamped from its sample count, anchored at the first packet ({positions})"
        ),
        Some(Stamp::Implausible { lag }) => warn!(
            "recorder",
            "own backend: game audio clock device: the QPC stamp is not this process's counter \
             ({:.1} ms from the moment the packet was taken, more than {} ms allows), so the \
             audio is stamped from its sample count, anchored at the first packet ({positions})",
            lag as f64 / 10_000.0,
            clock::STAMP_MAX_LAG / 10_000
        ),
        None => {}
    }
}
