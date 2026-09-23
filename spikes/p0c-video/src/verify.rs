//! Measuring the file, rather than believing the run that wrote it.
//!
//! `p0c-video` runs this on its own output after a clean run, and after the
//! child it killed at minute five; `--verify <file>` runs it on anything,
//! including a file left behind by a kill from Task Manager. It works on any
//! host, because it is box headers and an ffmpeg subprocess.
//!
//! What it establishes, row by row of DEVELOPMENT.md §16:
//!
//! - **WGC frames reach a fragmented MP4**: the `moov` declares fragments
//!   (`mvex`), there are `moof`/`mdat` pairs after it, and a decoder gets
//!   real frames out of the video track.
//! - **A file killed at minute five is playable**: the same, on the killed
//!   file, plus the app's own faststart remux succeeding on it. The number
//!   that goes with it is how much of the tail the kill cost.
//! - **Drift, from the file**: where the audio track ends against where the
//!   video track ends. The run pads the audio to the video's last tick before
//!   finalizing, so any offset here was introduced by the encoder or the
//!   muxer, not the capture.

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use crate::mp4;
use crate::probe::{self, Ffmpeg};

/// What the run knew when it last wrote to `<file>.progress`: the file's own
/// record of how far the sink writer had been fed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Fed {
    pub seconds: f64,
    pub video_ticks: u64,
    pub audio_samples: u64,
    pub audio_rate: u32,
    pub fps: u32,
}

pub fn progress_path(file: &Path) -> PathBuf {
    let mut name = file.as_os_str().to_owned();
    name.push(".progress");
    PathBuf::from(name)
}

/// One line, appended each second by the run: `t=12.00 ticks=720 ...`.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub fn format_fed(fed: &Fed) -> String {
    format!(
        "t={:.2} ticks={} fps={} audio_samples={} audio_rate={}",
        fed.seconds, fed.video_ticks, fed.fps, fed.audio_samples, fed.audio_rate
    )
}

pub fn parse_fed(line: &str) -> Option<Fed> {
    let mut fed = Fed::default();
    for pair in line.split_whitespace() {
        let (key, value) = pair.split_once('=')?;
        match key {
            "t" => fed.seconds = value.parse().ok()?,
            "ticks" => fed.video_ticks = value.parse().ok()?,
            "fps" => fed.fps = value.parse().ok()?,
            "audio_samples" => fed.audio_samples = value.parse().ok()?,
            "audio_rate" => fed.audio_rate = value.parse().ok()?,
            _ => {}
        }
    }
    (fed.fps > 0).then_some(fed)
}

fn read_fed(file: &Path) -> Option<Fed> {
    let reader = BufReader::new(File::open(progress_path(file)).ok()?);
    reader
        .lines()
        .map_while(Result::ok)
        .filter_map(|l| parse_fed(&l))
        .last()
}

/// Everything the check found, before it is turned into words.
#[derive(Default)]
pub struct Findings {
    pub summary: mp4::Summary,
    pub fed: Option<Fed>,
    pub ffmpeg: Option<String>,
    pub container_seconds: Option<f64>,
    pub streams: Vec<String>,
    pub video_frames: Option<u64>,
    pub video_end_us: Option<i64>,
    pub video_errors: Vec<String>,
    pub keyframes: Option<u64>,
    pub audio_end_us: Option<i64>,
    pub audio_errors: Vec<String>,
    pub remux_ok: Option<bool>,
    pub remux_errors: Vec<String>,
    pub remux_summary: Option<mp4::Summary>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    Playable,
    PlayableWithErrors,
    NotPlayable(&'static str),
    /// Structure is fine but nothing decoded it: no ffmpeg was found.
    Undecided,
}

impl Findings {
    pub fn verdict(&self) -> Verdict {
        if !self.summary.mvex {
            return Verdict::NotPlayable("not fragmented: moov has no mvex");
        }
        if !self.summary.structurally_playable() {
            return Verdict::NotPlayable("no complete moof+mdat fragment after the moov");
        }
        let Some(frames) = self.video_frames else {
            return Verdict::Undecided;
        };
        if frames == 0 {
            return Verdict::NotPlayable("ffmpeg decoded no video frames");
        }
        if self.remux_ok == Some(false) {
            return Verdict::NotPlayable("the faststart remux the app finalizes with failed");
        }
        if self.video_errors.is_empty() && self.audio_errors.is_empty() {
            Verdict::Playable
        } else {
            Verdict::PlayableWithErrors
        }
    }

    /// Audio end minus video end, in microseconds, where both decoded.
    pub fn av_end_offset_us(&self) -> Option<i64> {
        Some(self.audio_end_us? - self.video_end_us?)
    }
}

pub fn check(file: &Path, ffmpeg: Option<&Path>, expect_audio: bool) -> Result<Findings, String> {
    let mut handle =
        File::open(file).map_err(|e| format!("cannot open {}: {e}", file.display()))?;
    let summary = mp4::summarize(&mut handle)
        .map_err(|e| format!("reading the boxes of {} failed: {e}", file.display()))?;
    let mut findings = Findings {
        summary,
        fed: read_fed(file),
        ..Findings::default()
    };

    let tool = match probe::find(ffmpeg) {
        Ok(tool) => tool,
        Err(e) => {
            findings.ffmpeg = Some(format!("NOT FOUND - {e}"));
            return Ok(findings);
        }
    };
    findings.ffmpeg = Some(format!(
        "{} ({}): {}",
        tool.path.display(),
        tool.source,
        tool.version
    ));
    decode_all(&tool, file, expect_audio, &mut findings)?;
    Ok(findings)
}

fn decode_all(
    tool: &Ffmpeg,
    file: &Path,
    expect_audio: bool,
    f: &mut Findings,
) -> Result<(), String> {
    let banner = tool.banner(file)?;
    f.container_seconds = probe::parse_duration(&banner);
    f.streams = probe::stream_lines(&banner);

    let video = tool.decode(file, "0:v:0", false)?;
    f.video_frames = video.progress.frame;
    f.video_end_us = video.progress.out_time_us;
    f.video_errors = video.errors;
    if !video.ok {
        f.video_errors
            .push("(ffmpeg exited non-zero decoding the video track)".to_string());
    }

    let keys = tool.decode(file, "0:v:0", true)?;
    f.keyframes = keys.progress.frame;

    let has_audio = f.streams.iter().any(|s| s.contains("Audio:"));
    if expect_audio || has_audio {
        let audio = tool.decode(file, "0:a:0", false)?;
        f.audio_end_us = audio.progress.out_time_us;
        f.audio_errors = audio.errors;
        if !audio.ok {
            f.audio_errors
                .push("(ffmpeg exited non-zero decoding the audio track)".to_string());
        }
    }

    let mut remuxed = file.as_os_str().to_owned();
    remuxed.push(".remux.mp4");
    let remuxed = PathBuf::from(remuxed);
    let (ok, errors) = tool.remux(file, &remuxed)?;
    f.remux_ok = Some(ok);
    f.remux_errors = errors;
    if ok {
        if let Ok(mut out) = File::open(&remuxed) {
            f.remux_summary = mp4::summarize(&mut out).ok();
        }
        // It proved what it had to; it is the size of the original and
        // nobody needs a second copy.
        let _ = std::fs::remove_file(&remuxed);
    }
    Ok(())
}

fn seconds(us: Option<i64>) -> String {
    us.map_or("-".to_string(), |us| format!("{:.3} s", us as f64 / 1e6))
}

fn first_lines(lines: &[String], n: usize) -> String {
    let mut out: Vec<String> = lines.iter().take(n).map(|l| format!("      {l}")).collect();
    if lines.len() > n {
        out.push(format!("      ... and {} more", lines.len() - n));
    }
    out.join("\n")
}

/// The report block. `fps` is the grid the run wrote on.
pub fn render(file: &Path, f: &Findings, fps: u32) -> String {
    use std::fmt::Write as _;
    let s = &f.summary;
    let mut out = String::new();
    let _ = writeln!(out, "== file check: {} ==", file.display());
    let _ = writeln!(out, "size            {} bytes", s.file_len);
    let _ = writeln!(out, "boxes           {}", s.layout);
    let _ = writeln!(
        out,
        "fragmented      {} (moov {}, mvex {}, {} track(s))",
        if s.mvex { "yes" } else { "NO" },
        s.moov_offset
            .map_or("missing".to_string(), |o| format!("at {o}")),
        if s.mvex { "present" } else { "absent" },
        s.tracks
    );
    let _ = writeln!(
        out,
        "fragments       {} moof, {} complete moof+mdat; mfra {}",
        s.moof,
        s.complete_fragments,
        if s.mfra {
            "present (clean finalize)"
        } else {
            "absent"
        }
    );
    match &s.truncated {
        Some((kind, declared, present)) => {
            let _ = writeln!(
                out,
                "tail            '{kind}' cut short: {present} of {declared} bytes on disk"
            );
        }
        None if s.trailing_garbage > 0 => {
            let _ = writeln!(out, "tail            {} stray bytes", s.trailing_garbage);
        }
        None => {
            let _ = writeln!(out, "tail            every box complete");
        }
    }

    if let Some(fed) = &f.fed {
        let _ = writeln!(
            out,
            "fed to sink     {:.2} s: {} video ticks, {} audio samples (last .progress line)",
            fed.seconds, fed.video_ticks, fed.audio_samples
        );
    }

    let _ = writeln!(
        out,
        "ffmpeg          {}",
        f.ffmpeg.as_deref().unwrap_or("-")
    );
    if let Some(d) = f.container_seconds {
        let _ = writeln!(out, "container says  {d:.3} s");
    }
    for stream in &f.streams {
        let _ = writeln!(out, "  {stream}");
    }
    if f.video_frames.is_some() {
        let _ = writeln!(
            out,
            "video decoded   {} frames, to {} ({} error line(s))",
            f.video_frames.unwrap_or(0),
            seconds(f.video_end_us),
            f.video_errors.len()
        );
        if !f.video_errors.is_empty() {
            let _ = writeln!(out, "{}", first_lines(&f.video_errors, 5));
        }
        if let (Some(keys), Some(end)) = (f.keyframes, f.video_end_us) {
            let spacing = if keys > 0 {
                format!("one per {:.2} s", end as f64 / 1e6 / keys as f64)
            } else {
                "none".to_string()
            };
            let _ = writeln!(out, "keyframes       {keys} ({spacing})");
        }
        if let (Some(fed), Some(frames)) = (&f.fed, f.video_frames) {
            let lost = fed.video_ticks as i64 - frames as i64;
            let _ = writeln!(
                out,
                "tail lost       {lost} frame(s), {:.2} s: fed to the sink but not in the file",
                lost as f64 / f64::from(fps)
            );
        }
    }
    if f.audio_end_us.is_some() || !f.audio_errors.is_empty() {
        let _ = writeln!(
            out,
            "audio decoded   to {} ({} error line(s))",
            seconds(f.audio_end_us),
            f.audio_errors.len()
        );
        if !f.audio_errors.is_empty() {
            let _ = writeln!(out, "{}", first_lines(&f.audio_errors, 5));
        }
    }
    if let Some(offset) = f.av_end_offset_us() {
        let frames = offset as f64 * f64::from(fps) / 1e6;
        let _ = writeln!(
            out,
            "A/V end offset  {:+.1} ms = {frames:+.2} frame(s) (audio end minus video end; \
             AAC frames are 1024 samples, so +/-21 ms is the resolution)",
            offset as f64 / 1e3
        );
    }
    if let Some(ok) = f.remux_ok {
        let detail = match (&f.remux_summary, ok) {
            (Some(r), true) => format!(
                "ok; the result is {} with moov {}",
                if r.mvex {
                    "still fragmented"
                } else {
                    "an ordinary MP4"
                },
                match (r.moov_offset, r.first_mdat_offset) {
                    (Some(m), Some(d)) if m < d => "up front",
                    (Some(_), _) => "after the media",
                    (None, _) => "missing",
                }
            ),
            (None, true) => "ok".to_string(),
            (_, false) => format!("FAILED\n{}", first_lines(&f.remux_errors, 5)),
        };
        let _ = writeln!(out, "faststart remux {detail}");
    }

    let verdict = match f.verdict() {
        Verdict::Playable => "PLAYABLE".to_string(),
        Verdict::PlayableWithErrors => {
            "PLAYABLE, WITH DECODER ERRORS (see above; errors at the cut are expected)".to_string()
        }
        Verdict::NotPlayable(why) => format!("NOT PLAYABLE: {why}"),
        Verdict::Undecided => {
            "UNDECIDED: the structure is sound, but no ffmpeg was found to decode it".to_string()
        }
    };
    let _ = writeln!(out, "verdict         {verdict}");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fragmented() -> mp4::Summary {
        mp4::Summary {
            ftyp: true,
            moov_offset: Some(24),
            first_moof_offset: Some(900),
            mvex: true,
            moof: 10,
            mdat: 10,
            complete_fragments: 9,
            ..mp4::Summary::default()
        }
    }

    #[test]
    fn fed_round_trips() {
        let fed = Fed {
            seconds: 300.25,
            video_ticks: 18_015,
            audio_samples: 14_412_000,
            audio_rate: 48_000,
            fps: 60,
        };
        assert_eq!(parse_fed(&format_fed(&fed)), Some(fed));
        assert_eq!(parse_fed("garbage"), None);
    }

    #[test]
    fn the_verdict_needs_structure_and_frames() {
        let mut f = Findings {
            summary: fragmented(),
            ..Findings::default()
        };
        assert_eq!(f.verdict(), Verdict::Undecided);
        f.video_frames = Some(0);
        assert!(matches!(f.verdict(), Verdict::NotPlayable(_)));
        f.video_frames = Some(17_900);
        f.remux_ok = Some(true);
        assert_eq!(f.verdict(), Verdict::Playable);
        f.video_errors = vec!["[h264] error while decoding MB".into()];
        assert_eq!(f.verdict(), Verdict::PlayableWithErrors);
        f.remux_ok = Some(false);
        assert!(matches!(f.verdict(), Verdict::NotPlayable(_)));
    }

    #[test]
    fn an_unfragmented_file_fails_before_decoding() {
        let f = Findings {
            summary: mp4::Summary {
                mvex: false,
                ..fragmented()
            },
            video_frames: Some(100),
            ..Findings::default()
        };
        assert!(matches!(f.verdict(), Verdict::NotPlayable(_)));
    }

    #[test]
    fn the_offset_is_audio_minus_video() {
        let f = Findings {
            video_end_us: Some(600_000_000),
            audio_end_us: Some(600_021_333),
            ..Findings::default()
        };
        assert_eq!(f.av_end_offset_us(), Some(21_333));
    }
}
