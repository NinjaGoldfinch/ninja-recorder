//! Every written audio track while a recording runs: each source's thread,
//! the packets each sends, and the [`TrackMix`] that aligns and mixes each
//! track from them once the origin is known. The session thread holds one of
//! these and calls it on each pass of its loop; the decisions are all in
//! `own::mix`, `own::feed` and `own::plan`, which are tested on any host.
//!
//! **Each source is captured once and fed to every track that sums it**
//! (#239). Game + mic + Discord opens three sources and mixes four tracks:
//! track 0 sums all three, and each stem is one of them alone
//! (`own::mix::TrackMix`).
//!
//! **A source that cannot open costs itself, not the recording.** No
//! microphone, Discord not running, a game tree that cannot be found: each is
//! logged, left out, and dropped from the layout `stop` reports along with
//! any stem it alone fed (`plan::realised_layout`). Only when nothing opens is
//! the file video only. **A source that dies mid-recording** (a microphone
//! unplugged) is logged once, with the reason its thread gave, and is silence
//! from then on, in the mix and in its stem: the mixer's watermark is what
//! stops it holding anything up.

use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::HWND;

use super::super::process;
use super::{SAMPLE_RATE, Source, Summary, Target};
use crate::recorder::audio::{AudioLayout, AudioSourceKind};
use crate::recorder::own::clock::{self, AudioClock};
use crate::recorder::own::feed::{Feed, Packet};
use crate::recorder::own::mix::TrackMix;
use crate::recorder::own::plan::{self, CapturePlan, source_name};
use crate::recorder::own::root::{self, Proc};
use crate::{info, warn};

/// How long a stop waits for the audio packets of the last video tick,
/// which are still in flight when the stop arrives. Whatever has not come
/// by then is padded with silence.
const TAIL_WAIT: Duration = Duration::from_millis(200);

/// One opened source.
struct Input {
    /// `game`, `microphone`, `desktop`, or the application's executable.
    name: String,
    /// `None` once stopped. Dropping the tracks stops every one.
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

/// Every audio track of a recording.
pub struct AudioTracks {
    /// The sources that opened, in the realised layout's source order.
    inputs: Vec<Input>,
    /// The realised layout: which of `inputs` each track sums, and its label.
    layout: AudioLayout,
    /// Made when the origin arrives; until then packets wait in the channels.
    mix: Option<TrackMix>,
}

/// Resolves `kind` to what its thread captures. `procs` is the process
/// snapshot, taken on first use and shared by every process source.
fn target(
    kind: &AudioSourceKind,
    hwnd: HWND,
    procs: &mut Option<Vec<Proc>>,
) -> Result<Target, String> {
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
            Ok(Target::Process(root.pid))
        }
        AudioSourceKind::Application { exe } => {
            let root = root::application_root(&snapshot()?, exe)?;
            info!("recorder", "own backend: {exe} audio from PID {}, {}", root.pid, root.how);
            Ok(Target::Process(root.pid))
        }
        AudioSourceKind::Microphone { device_id } => Ok(Target::Microphone(device_id.clone())),
        AudioSourceKind::Desktop => Ok(Target::Desktop),
    }
}

impl AudioTracks {
    /// Opens every source `plan` names, for the game window `hwnd`. Returns
    /// the tracks, or `None` if no source opened, and the layout the file
    /// will hold: `plan`'s, less whatever did not open.
    pub fn start(hwnd: HWND, plan: &CapturePlan) -> (Option<AudioTracks>, AudioLayout) {
        let mut procs = None;
        let mut inputs = Vec::new();
        let mut opened = Vec::with_capacity(plan.sources.len());
        for kind in &plan.sources {
            let name = source_name(kind);
            let (tx, packets) = channel();
            let source = target(kind, hwnd, &mut procs)
                .and_then(|target| super::start(&name, target, tx));
            match source {
                Ok(source) => {
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
        // The realised layout's sources are the ones that opened, in plan
        // order, which is the order `inputs` was filled in: source `i` of the
        // layout is `inputs[i]`.
        let tracks = (!inputs.is_empty()).then(|| AudioTracks {
            inputs,
            layout: layout.clone(),
            mix: None,
        });
        (tracks, layout)
    }

    /// The origin has arrived: packets can be placed from here on.
    pub fn begin(&mut self, origin: i64) {
        self.mix = Some(TrackMix::new(SAMPLE_RATE, origin, &self.layout));
    }

    /// Moves every packet that has arrived into the mixdown of each track
    /// that sums its source, and notices a source that has stopped on its
    /// own.
    fn receive(&mut self) {
        let Some(mix) = self.mix.as_mut() else {
            return;
        };
        for (i, input) in self.inputs.iter_mut().enumerate() {
            loop {
                match input.packets.try_recv() {
                    Ok(packet) => mix.push(i, packet),
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
                                 did ({why}); it is silence in every track it feeds from here",
                                input.name
                            );
                        }
                        break;
                    }
                }
            }
        }
    }

    /// One pass: receives what has arrived and writes whatever each track's
    /// mix has ready, never past `video_end_rel`, as `write(track, pcm,
    /// position)`. `now_rel` is the performance counter now; both are
    /// relative to the origin.
    pub fn write<W>(&mut self, video_end_rel: i64, now_rel: i64, write: &mut W) -> Result<(), String>
    where
        W: FnMut(usize, &[i16], u64) -> Result<(), String>,
    {
        self.receive();
        match self.mix.as_mut() {
            Some(mix) => mix.write(video_end_rel, now_rel, write),
            None => Ok(()),
        }
    }

    /// Ends every track at `end_rel`, the end of the last video tick: waits
    /// briefly for the packets still in flight, stops every source, writes
    /// what came, pads each source and each mix to `end_rel`, and logs what
    /// each source's clock did and what each track's mixer did. Errors are
    /// logged rather than returned, because the file is finalized either way.
    /// A recording with nothing to write to drops the tracks instead, which
    /// stops the sources.
    pub fn finish<W>(mut self, end_rel: i64, write: &mut W)
    where
        W: FnMut(usize, &[i16], u64) -> Result<(), String>,
    {
        if self.mix.is_some() {
            let deadline = Instant::now() + TAIL_WAIT;
            loop {
                self.receive();
                let reached = self.inputs.iter().enumerate().all(|(i, input)| {
                    input.ended
                        || self
                            .mix
                            .as_ref()
                            .and_then(|mix| mix.feed(i))
                            .is_none_or(|feed| feed.reaches(end_rel))
                });
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
        let Some(mut mix) = self.mix.take() else {
            return;
        };

        let finished = mix.finish(end_rel, write);
        for (t, result) in finished.iter().enumerate() {
            if let Err(e) = result {
                warn!(
                    "recorder",
                    "own backend: the end of audio track {t} ({}) was not written: {e}",
                    self.layout.tracks[t].label
                );
            }
        }
        // Each source once, from the first track that sums it.
        for (i, input) in self.inputs.iter_mut().enumerate() {
            let Some((t, lane)) = mix.first_lane(i) else { continue };
            let pad = finished[t].as_ref().ok().and_then(|p| p.get(lane)).copied().unwrap_or(0);
            let mixdown = mix.track(t);
            let (late, missing) = (mixdown.mixer().late(lane), mixdown.mixer().missing(lane));
            log_source(input, mixdown.feed(lane), pad, late, missing);
        }
        for (t, track) in self.layout.tracks.iter().enumerate() {
            let names: Vec<&str> = track
                .sources
                .iter()
                .filter_map(|&s| self.inputs.get(s).map(|input| input.name.as_str()))
                .collect();
            let mixer = mix.track(t).mixer();
            info!(
                "recorder",
                "own backend: audio track {t} ({}), {}: {} blocks, {} released by the watermark \
                 with a source short, {} samples clipped; {:.3} s written",
                track.label,
                names.join(" + "),
                mixer.stats.blocks,
                mixer.stats.released,
                mixer.stats.clipped,
                mixer.emitted() as f64 / f64::from(SAMPLE_RATE)
            );
        }
    }
}

impl Drop for AudioTracks {
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
