//! Track 0 while a recording runs: every source's thread, the packets each
//! sends, and the [`Mixdown`] that aligns and mixes them once the origin is
//! known. The session thread holds one of these and calls it on each pass of
//! its loop; the decisions are all in `own::mix`, `own::feed` and
//! `own::plan`, which are tested on any host.
//!
//! **A source that cannot open costs itself, not the recording.** No
//! microphone, Discord not running, a game tree that cannot be found: each is
//! logged, left out, and dropped from the layout `stop` reports
//! (`plan::realised_layout`). Only when nothing opens is the file video only.
//! **A source that dies mid-recording** (a microphone unplugged) is logged
//! once, with the reason its thread gave, and is silence from then on: the
//! mixer's watermark is what stops it holding anything up.

use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::HWND;

use super::super::process;
use super::{SAMPLE_RATE, Source, Summary, Target};
use crate::recorder::audio::{AudioLayout, AudioSourceKind};
use crate::recorder::own::clock::{self, AudioClock};
use crate::recorder::own::feed::{Feed, Packet};
use crate::recorder::own::mix::Mixdown;
use crate::recorder::own::plan::{self, CapturePlan};
use crate::recorder::own::root::{self, Proc};
use crate::recorder::own::stats::{AudioStop, Opened, SourceStop};
use crate::{info, warn};

/// How long a stop waits for the audio packets of the last video tick,
/// which are still in flight when the stop arrives. Whatever has not come
/// by then is padded with silence.
const TAIL_WAIT: Duration = Duration::from_millis(200);

/// One opened source.
struct Input {
    /// `game`, `microphone`, `desktop`, or the application's executable.
    name: String,
    /// `None` once stopped. Dropping a `MixTrack` stops every one.
    source: Option<Source>,
    packets: Receiver<Packet>,
    /// Set once the channel has closed, so a source that died is logged once.
    ended: bool,
    /// How the thread ended, if it has: kept for the line at stop.
    summary: Option<Result<Summary, String>>,
}

impl Input {
    fn stop(&mut self) {
        if let Some(source) = self.source.take() {
            self.summary = Some(source.stop());
        }
    }
}

/// Track 0 of a recording.
pub struct MixTrack {
    inputs: Vec<Input>,
    /// Made when the origin arrives; until then packets wait in the channels.
    mixdown: Option<Mixdown>,
}

/// What a source kind is called in the log and its thread's name.
fn name_of(kind: &AudioSourceKind) -> String {
    match kind {
        AudioSourceKind::Game => "game".to_string(),
        AudioSourceKind::Microphone { .. } => "microphone".to_string(),
        AudioSourceKind::Desktop => "desktop".to_string(),
        AudioSourceKind::Application { exe } => exe.clone(),
    }
}

/// Resolves `kind` to what its thread captures, and says what that is for
/// the start line. `procs` is the process snapshot, taken on first use and
/// shared by every process source.
fn target(
    kind: &AudioSourceKind,
    hwnd: HWND,
    procs: &mut Option<Vec<Proc>>,
) -> Result<(Target, String), String> {
    let mut snapshot = || -> Result<Vec<Proc>, String> {
        if procs.is_none() {
            *procs = Some(process::snapshot()?);
        }
        Ok(procs.clone().unwrap_or_default())
    };
    match kind {
        AudioSourceKind::Game => {
            let root = root::game_root(&snapshot()?, process::window_owner(hwnd))?;
            info!("recorder", "own backend: game audio from PID {}, {}", root.pid, root.how);
            Ok((Target::Process(root.pid), format!("PID {} ({})", root.pid, root.how)))
        }
        AudioSourceKind::Application { exe } => {
            let root = root::application_root(&snapshot()?, exe)?;
            info!("recorder", "own backend: {exe} audio from PID {}, {}", root.pid, root.how);
            Ok((Target::Process(root.pid), format!("PID {} ({})", root.pid, root.how)))
        }
        AudioSourceKind::Microphone { device_id } => Ok((
            Target::Microphone(device_id.clone()),
            device_id.clone().unwrap_or_else(|| "default".to_string()),
        )),
        AudioSourceKind::Desktop => Ok((Target::Desktop, "default output".to_string())),
    }
}

impl MixTrack {
    /// Opens every source `plan` names, for the game window `hwnd`. Returns
    /// the track, or `None` if no source opened; the layout the file will
    /// hold, `plan`'s less whatever did not open; and what became of each
    /// source, for the start line (`own::stats`).
    pub fn start(
        hwnd: HWND,
        plan: &CapturePlan,
    ) -> (Option<MixTrack>, AudioLayout, Vec<Opened>) {
        let mut procs = None;
        let mut inputs = Vec::new();
        let mut opened = Vec::with_capacity(plan.sources.len());
        let mut report = Vec::with_capacity(plan.sources.len());
        for kind in &plan.sources {
            let name = name_of(kind);
            let (tx, packets) = channel();
            let source = target(kind, hwnd, &mut procs).and_then(|(target, what)| {
                super::start(&name, target, tx).map(|source| (source, what))
            });
            report.push(Opened {
                name: name.clone(),
                outcome: source.as_ref().map(|(_, what)| what.clone()).map_err(Clone::clone),
            });
            match source {
                Ok((source, _)) => {
                    inputs.push(Input {
                        name,
                        source: Some(source),
                        packets,
                        ended: false,
                        summary: None,
                    });
                    opened.push(true);
                }
                Err(e) => {
                    warn!(
                        "recorder",
                        "own backend: no {name} audio; it is left out of the recording: {e}"
                    );
                    opened.push(false);
                }
            }
        }
        let layout = plan::realised_layout(&plan.layout(), &opened);
        // With track 0 the only one written, every source that opened feeds
        // it, so the mixer's lanes are the inputs in order. #239 mixes each
        // track from `layout.tracks[i].sources` instead.
        let track = (!inputs.is_empty()).then_some(MixTrack { inputs, mixdown: None });
        (track, layout, report)
    }

    /// The sources in the mix, by name, for the log.
    pub fn names(&self) -> String {
        let names: Vec<&str> = self.inputs.iter().map(|i| i.name.as_str()).collect();
        names.join(" + ")
    }

    /// The origin has arrived: packets can be placed from here on.
    pub fn begin(&mut self, origin: i64) {
        self.mixdown = Some(Mixdown::new(SAMPLE_RATE, origin, self.inputs.len()));
    }

    /// Moves every packet that has arrived into the mixdown, and notices a
    /// source that has stopped on its own.
    fn receive(&mut self) {
        let Some(mixdown) = self.mixdown.as_mut() else {
            return;
        };
        for (i, input) in self.inputs.iter_mut().enumerate() {
            loop {
                match input.packets.try_recv() {
                    Ok(packet) => mixdown.push(i, packet),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        if !input.ended {
                            input.ended = true;
                            // The thread has returned, so this join is
                            // immediate, and it is what says why.
                            input.stop();
                            let why = match &input.summary {
                                Some(Err(e)) => e.clone(),
                                _ => "it stopped delivering".to_string(),
                            };
                            warn!(
                                "recorder",
                                "own backend: the {} audio capture ended before the recording \
                                 did ({why}); it is silence in the mix from here",
                                input.name
                            );
                        }
                        break;
                    }
                }
            }
        }
    }

    /// One pass: receives what has arrived and writes whatever the mix has
    /// ready, never past `video_end_rel`. `now_rel` is the performance
    /// counter now; both are relative to the origin.
    pub fn write<W>(&mut self, video_end_rel: i64, now_rel: i64, write: &mut W) -> Result<(), String>
    where
        W: FnMut(&[i16], u64) -> Result<(), String>,
    {
        self.receive();
        match self.mixdown.as_mut() {
            Some(mixdown) => mixdown.write(video_end_rel, now_rel, write),
            None => Ok(()),
        }
    }

    /// Ends track 0 at `end_rel`, the end of the last video tick: waits
    /// briefly for the packets still in flight, stops every source, writes
    /// what came, pads each source and the mix to `end_rel`, and logs what
    /// each source's clock did and what the mixer did. Errors are logged
    /// rather than returned, because the file is finalized either way. A
    /// recording with nothing to write to drops the track instead, which
    /// stops the sources. Returns what the stop line sums up, if anything
    /// was placed.
    pub fn finish<W>(mut self, end_rel: i64, write: &mut W) -> Option<AudioStop>
    where
        W: FnMut(&[i16], u64) -> Result<(), String>,
    {
        if self.mixdown.is_some() {
            let deadline = Instant::now() + TAIL_WAIT;
            loop {
                self.receive();
                let reached = match &self.mixdown {
                    Some(mixdown) => self
                        .inputs
                        .iter()
                        .enumerate()
                        .all(|(i, input)| input.ended || mixdown.feed(i).reaches(end_rel)),
                    None => true,
                };
                if reached || Instant::now() >= deadline {
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        for input in &mut self.inputs {
            input.stop();
            // Stopped on purpose: the channel closing now is not the source
            // dying.
            input.ended = true;
        }
        self.receive();

        let mut mixdown = self.mixdown.take()?;
        let pads = match mixdown.finish(end_rel, write) {
            Ok(pads) => pads,
            Err(e) => {
                warn!("recorder", "own backend: the end of the audio was not written: {e}");
                Vec::new()
            }
        };
        let mut sources = Vec::with_capacity(self.inputs.len());
        for (i, input) in self.inputs.iter_mut().enumerate() {
            let failed = matches!(input.summary, Some(Err(_)));
            sources.push(SourceStop::from_aligner(&input.name, mixdown.feed(i).aligner(), failed));
            let pad = pads.get(i).copied().unwrap_or(0);
            let mixer = mixdown.mixer();
            log_source(input, mixdown.feed(i), pad, mixer.late(i), mixer.missing(i));
        }
        let mixer = mixdown.mixer();
        info!(
            "recorder",
            "own backend: track 0, the mix of {}: {} blocks, {} released by the watermark with a \
             source short, {} samples clipped; {:.3} s written",
            self.names(),
            mixer.stats.blocks,
            mixer.stats.released,
            mixer.stats.clipped,
            mixer.emitted() as f64 / f64::from(SAMPLE_RATE)
        );
        Some(AudioStop { sources, clipped: mixer.stats.clipped })
    }
}

impl Drop for MixTrack {
    fn drop(&mut self) {
        for input in &mut self.inputs {
            input.stop();
        }
    }
}

/// One source's line at stop: its clock, what its aligner did, what the
/// mixer did with it, and what its capture counted.
fn log_source(input: &mut Input, feed: &Feed, padded: u64, late: u64, missing: u64) {
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
    let source_line = match input.summary.take() {
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
        "own backend: {} audio {clock_line}; gaps {} ({:.1} ms), overlaps {} ({:.1} ms), held \
         {} times ({:.1} ms); lead silence {:.1} ms, lead dropped {:.1} ms; padded {:.1} ms at the \
         end; {:.3} s written; in the mix, {:.1} ms late and dropped, {:.1} ms silent for want of \
         a packet; {source_line}",
        input.name,
        s.gaps,
        ms(s.gap_samples as i64),
        s.overlaps,
        ms(s.overlap_samples as i64),
        s.holds,
        ms(s.held_samples as i64),
        ms(s.lead_silence as i64),
        ms(s.lead_dropped as i64),
        ms(padded as i64),
        a.written() as f64 / f64::from(rate),
        ms(late as i64),
        ms(missing as i64)
    );
}
