//! The own backend's session summary: one line in `daemon.log` when a
//! recording starts and one when it stops, so a run can be judged from the
//! log alone and #11's backend comparison filled in from it (DEVELOPMENT.md
//! §13, "The own backend's summary lines").
//!
//! Everything here is plain data and rendering. `own/win/` fills the structs
//! with what the session already knows (the adapter, the encoder the sink
//! writer loaded, the sources that opened, each source's aligner, the ticks
//! it wrote) and logs the result; the wording is a unit test on any host.
//!
//! The detailed lines the session logs as it goes stay: they are what the
//! verification rows ask to have pasted. These sum them up.

use std::path::Path;
use std::time::Duration;

use super::clock::{self, Aligner, AudioClock};
use super::status::Status;
use crate::recorder::audio::AudioLayout;

/// The file a line names: its file name, or the whole path if it has none.
fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into())
}

/// One source the plan named, and what became of it at start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Opened {
    /// `game`, `microphone`, `desktop`, or the application's executable.
    pub name: String,
    /// What it captures (`PID 1234, the game window's owner, …`, or the
    /// endpoint) when it opened; why not when it did not.
    pub outcome: Result<String, String>,
}

/// The start line: `own: recording <file>: <W>x<H> from <adapter>, encoder
/// <name> (<hardware|software fallback: reason>), sources: …; tracks: …`.
pub fn render_start(
    path: &Path,
    (width, height): (u32, u32),
    adapter: &str,
    status: &Status,
    sources: &[Opened],
    layout: &AudioLayout,
) -> String {
    let encoder = match status {
        Status::Ready { encoder } => format!("{encoder} (hardware)"),
        Status::Software { encoder, reason } => format!("{encoder} (software fallback: {reason})"),
        // Neither reaches a recording; named rather than hidden if one does.
        Status::Unavailable { reason } => format!("none (unavailable: {reason})"),
        Status::Idle => "none (idle)".to_string(),
    };
    let sources = if sources.is_empty() {
        "none planned".to_string()
    } else {
        let each: Vec<String> = sources
            .iter()
            .map(|s| match &s.outcome {
                Ok(what) => format!("{}={what}", s.name),
                Err(why) => format!("{}=failed ({why})", s.name),
            })
            .collect();
        each.join(", ")
    };
    let tracks = if layout.tracks.is_empty() {
        "none (video only)".to_string()
    } else {
        let labels: Vec<&str> = layout.tracks.iter().map(|t| t.label.as_str()).collect();
        labels.join(", ")
    };
    format!(
        "own: recording {}: {width}x{height} from {adapter}, encoder {encoder}, sources: \
         {sources}; tracks: {tracks}",
        file_name(path)
    )
}

/// The video's cadence: every tick written, how many of them repeated the
/// frame before, and how late the latest one was written.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Cadence {
    pub ticks: u64,
    /// Ticks that showed the same picture as the tick before: no new frame
    /// from WGC in between (a static or minimised window, or a capture that
    /// cannot keep up).
    pub repeated: u64,
    /// The most any tick was written after its own time, in 100 ns units.
    pub worst_late: i64,
    /// No new picture since the last tick. False at first, because the first
    /// tick shows the first frame.
    stale: bool,
}

impl Cadence {
    /// A new picture is in the slot the next tick shows: a frame from WGC,
    /// or black.
    pub fn frame(&mut self) {
        self.stale = false;
    }

    /// One tick written, `late` (100 ns units) after the time it stands for.
    pub fn tick(&mut self, late: i64) {
        self.ticks += 1;
        self.repeated += u64::from(self.stale);
        self.stale = true;
        self.worst_late = self.worst_late.max(late);
    }
}

/// One audio source at stop, from its aligner.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceStop {
    pub name: String,
    pub clock: AudioClock,
    /// The device clock against QPC, in ppm. Only measured on the QPC clock;
    /// on the device clock the audio is stamped from its own sample count,
    /// so there is nothing to compare.
    pub raw_ppm: Option<f64>,
    /// Single frames dropped or repeated to hold the audio on QPC.
    pub slips: u64,
    /// Holes past the gap threshold, filled with silence.
    pub gaps: u64,
    /// Times silence was written because no packet had come.
    pub holds: u64,
    /// The capture ended on an error of its own before the recording did.
    pub ended_early: bool,
}

impl SourceStop {
    pub fn from_aligner(name: &str, aligner: &Aligner, ended_early: bool) -> SourceStop {
        let s = &aligner.stats;
        SourceStop {
            name: name.to_string(),
            clock: aligner.clock(),
            raw_ppm: aligner.raw_ppm(),
            slips: s.slips_dropped + s.slips_repeated,
            gaps: s.gaps,
            holds: s.holds,
            ended_early,
        }
    }
}

/// Track 0 at stop: its sources, and what the mixer clipped.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AudioStop {
    pub sources: Vec<SourceStop>,
    pub clipped: u64,
}

/// What the stop line reports, filled in as the recording runs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stop {
    pub cadence: Cadence,
    /// `None` for a video-only recording, or one that never got its origin.
    pub audio: Option<AudioStop>,
}

/// The stop line: `own: stopped <file>: <s> s, <n> ticks, <p>% repeated,
/// worst tick <x> f late; per source: …; mix clipped <n>; <MB> MB; finalize
/// <ok|failed>`.
///
/// `bytes` is the file's size after the finalize, if it could be read.
pub fn render_stop(
    path: &Path,
    fps: u32,
    stop: &Stop,
    finalized: &Result<(), String>,
    bytes: Option<u64>,
) -> String {
    let c = &stop.cadence;
    let seconds = clock::tick_time(c.ticks, fps) as f64 / clock::HNS_PER_SECOND as f64;
    let video = if c.ticks == 0 {
        "0.000 s, no ticks written".to_string()
    } else {
        format!(
            "{seconds:.3} s, {} ticks, {:.2}% repeated, worst tick {:.2} f late",
            c.ticks,
            c.repeated as f64 * 100.0 / c.ticks as f64,
            clock::hns_to_frames(c.worst_late, fps)
        )
    };
    let audio = match &stop.audio {
        None => "no audio".to_string(),
        Some(audio) => {
            let each: Vec<String> = audio
                .sources
                .iter()
                .map(|s| {
                    let raw = match (s.clock, s.raw_ppm) {
                        (AudioClock::Qpc, Some(ppm)) => format!(" raw={ppm:+.2} ppm"),
                        (AudioClock::Qpc, None) => " raw=unmeasured".to_string(),
                        (AudioClock::Device, _) => String::new(),
                    };
                    format!(
                        "{} clock={}{raw} slips={} gaps={} holds={}{}",
                        s.name,
                        s.clock.name(),
                        s.slips,
                        s.gaps,
                        s.holds,
                        if s.ended_early { " ended early" } else { "" }
                    )
                })
                .collect();
            let each = if each.is_empty() { "none".to_string() } else { each.join(", ") };
            format!("per source: {each}; mix clipped {}", audio.clipped)
        }
    };
    let size = bytes.map_or_else(String::new, |b| format!("; {:.1} MB", b as f64 / 1e6));
    let finalize = match finalized {
        Ok(()) => "ok",
        Err(_) => "failed",
    };
    format!("own: stopped {}: {video}; {audio}{size}; finalize {finalize}", file_name(path))
}

/// The line after the stop, from the daemon's side: the faststart remux.
/// `None` is a remux that was not attempted (no ffmpeg, or a session that
/// never answered and may still be writing the file).
pub fn render_remux(path: &Path, remux: Option<&(Result<(), String>, Duration)>) -> String {
    let result = match remux {
        None => "skipped".to_string(),
        Some((Ok(()), took)) => format!("ok in {} ms", took.as_millis()),
        Some((Err(_), took)) => format!("failed in {} ms, kept unseekable", took.as_millis()),
    };
    format!("own: remux {}: {result}", file_name(path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recorder::audio::{AudioSourceKind, AudioTrackSpec};

    fn path() -> &'static Path {
        Path::new("C:/Videos/League/2026-09-26_12-00-00.mp4")
    }

    fn layout(labels: &[&str]) -> AudioLayout {
        AudioLayout {
            sources: vec![AudioSourceKind::Game],
            tracks: labels
                .iter()
                .map(|l| AudioTrackSpec { label: l.to_string(), sources: vec![0] })
                .collect(),
        }
    }

    #[test]
    fn start_names_the_hardware_encoder_every_source_and_the_tracks() {
        let sources = vec![
            Opened {
                name: "game".into(),
                outcome: Ok("PID 4242 (the game window's owner, named League of Legends.exe)"
                    .into()),
            },
            Opened { name: "microphone".into(), outcome: Ok("default".into()) },
            Opened { name: "desktop".into(), outcome: Ok("default output".into()) },
            Opened {
                name: "Discord.exe".into(),
                outcome: Ok("PID 77 (the top of Discord.exe's tree (6 processes))".into()),
            },
        ];
        let line = render_start(
            path(),
            (1920, 1080),
            "NVIDIA GeForce RTX 3070",
            &Status::Ready { encoder: "NVIDIA H.264 Encoder MFT [VEN_10DE]".into() },
            &sources,
            &layout(&["Mix"]),
        );
        assert_eq!(
            line,
            "own: recording 2026-09-26_12-00-00.mp4: 1920x1080 from NVIDIA GeForce RTX 3070, \
             encoder NVIDIA H.264 Encoder MFT [VEN_10DE] (hardware), sources: game=PID 4242 \
             (the game window's owner, named League of Legends.exe), microphone=default, \
             desktop=default output, Discord.exe=PID 77 (the top of Discord.exe's tree (6 \
             processes)); tracks: Mix"
        );
    }

    #[test]
    fn start_says_software_fallback_and_why() {
        let line = render_start(
            path(),
            (1280, 720),
            "Microsoft Basic Render Driver",
            &Status::Software {
                encoder: "the software H.264 MFT".into(),
                reason: "no hardware H.264 encoder matches a hardware adapter".into(),
            },
            &[Opened { name: "game".into(), outcome: Ok("PID 1".into()) }],
            &layout(&["Game"]),
        );
        assert!(line.contains(
            "encoder the software H.264 MFT (software fallback: no hardware H.264 encoder \
             matches a hardware adapter)"
        ));
    }

    #[test]
    fn start_marks_a_source_that_failed_to_open_and_a_video_only_file() {
        let line = render_start(
            path(),
            (1920, 1080),
            "AMD Radeon RX 6800",
            &Status::Ready { encoder: "AMDh264Encoder".into() },
            &[Opened {
                name: "Discord.exe".into(),
                outcome: Err("no Discord.exe process is running".into()),
            }],
            &AudioLayout { sources: Vec::new(), tracks: Vec::new() },
        );
        assert!(line.ends_with(
            "sources: Discord.exe=failed (no Discord.exe process is running); tracks: none \
             (video only)"
        ));
    }

    #[test]
    fn start_with_no_sources_planned() {
        let line = render_start(
            path(),
            (2, 2),
            "a",
            &Status::Ready { encoder: "e".into() },
            &[],
            &AudioLayout { sources: Vec::new(), tracks: Vec::new() },
        );
        assert!(line.contains("sources: none planned; tracks: none (video only)"));
    }

    #[test]
    fn cadence_counts_repeats_and_the_worst_late_tick() {
        let mut c = Cadence::default();
        c.tick(1_000); // the first frame, fresh
        c.tick(2_000); // nothing new: repeated
        c.frame();
        c.frame(); // two frames between ticks still make one fresh tick
        c.tick(50_000);
        c.tick(0); // repeated
        assert_eq!((c.ticks, c.repeated, c.worst_late), (4, 2, 50_000));
    }

    fn qpc(name: &str) -> SourceStop {
        SourceStop {
            name: name.into(),
            clock: AudioClock::Qpc,
            raw_ppm: Some(-12.345),
            slips: 7,
            gaps: 1,
            holds: 3,
            ended_early: false,
        }
    }

    #[test]
    fn stop_sums_up_the_video_and_each_source() {
        let stop = Stop {
            cadence: Cadence { ticks: 6000, repeated: 60, worst_late: 83_333, stale: true },
            audio: Some(AudioStop {
                sources: vec![
                    qpc("game"),
                    SourceStop {
                        name: "microphone".into(),
                        clock: AudioClock::Device,
                        raw_ppm: None,
                        slips: 0,
                        gaps: 2,
                        holds: 0,
                        ended_early: true,
                    },
                ],
                clipped: 12,
            }),
        };
        let line = render_stop(path(), 60, &stop, &Ok(()), Some(104_857_600));
        assert_eq!(
            line,
            "own: stopped 2026-09-26_12-00-00.mp4: 100.000 s, 6000 ticks, 1.00% repeated, worst \
             tick 0.50 f late; per source: game clock=qpc raw=-12.35 ppm slips=7 gaps=1 holds=3, \
             microphone clock=device slips=0 gaps=2 holds=0 ended early; mix clipped 12; 104.9 \
             MB; finalize ok"
        );
    }

    #[test]
    fn stop_with_zero_ticks_no_audio_and_a_failed_finalize() {
        let line = render_stop(path(), 60, &Stop::default(), &Err("drain failed".into()), None);
        assert_eq!(
            line,
            "own: stopped 2026-09-26_12-00-00.mp4: 0.000 s, no ticks written; no audio; \
             finalize failed"
        );
    }

    #[test]
    fn stop_says_when_raw_drift_was_not_measured() {
        let mut source = qpc("game");
        source.raw_ppm = None;
        let stop = Stop {
            cadence: Cadence { ticks: 1, ..Cadence::default() },
            audio: Some(AudioStop { sources: vec![source], clipped: 0 }),
        };
        let line = render_stop(path(), 60, &stop, &Ok(()), Some(0));
        assert!(line.contains("game clock=qpc raw=unmeasured slips=7"), "{line}");
        assert!(line.contains("0.017 s, 1 ticks, 0.00% repeated"), "{line}");
    }

    #[test]
    fn remux_ok_failed_and_skipped() {
        let took = Duration::from_millis(812);
        assert_eq!(
            render_remux(path(), Some(&(Ok(()), took))),
            "own: remux 2026-09-26_12-00-00.mp4: ok in 812 ms"
        );
        assert_eq!(
            render_remux(path(), Some(&(Err("ffmpeg exited 1".into()), took))),
            "own: remux 2026-09-26_12-00-00.mp4: failed in 812 ms, kept unseekable"
        );
        assert_eq!(render_remux(path(), None), "own: remux 2026-09-26_12-00-00.mp4: skipped");
    }

    #[test]
    fn from_aligner_sums_both_kinds_of_slip() {
        let mut aligner = Aligner::new(48_000, AudioClock::Qpc);
        aligner.stats.slips_dropped = 2;
        aligner.stats.slips_repeated = 3;
        aligner.stats.gaps = 1;
        aligner.stats.holds = 4;
        let s = SourceStop::from_aligner("game", &aligner, false);
        assert_eq!((s.slips, s.gaps, s.holds, s.raw_ppm), (5, 1, 4, None));
    }
}
