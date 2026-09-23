//! The other half of "playable": does a real decoder get through the file?
//!
//! Box structure says a player *could* read the file; decoding it says one
//! did. The tool is `ffmpeg`, not `ffprobe`, because `ffmpeg.exe` is the one
//! the app bundles (`src-tauri/src/probe.rs` says why ffprobe is not), so the
//! run needs nothing installed beyond the app itself.
//!
//! The spike looks for it in this order: `--ffmpeg <path>`, the installed
//! app's copy at `%LOCALAPPDATA%\ninja-recorder\libobs\ffmpeg.exe`, then
//! `ffmpeg` on `PATH`. Whichever it used is printed with its version line.
//!
//! Every measurement is one `ffmpeg ... -f null -` decode with
//! `-progress pipe:1`, whose `key=value` lines are parsed below. Spawning is
//! plain `std::process`, so this module compiles, and its parsing is tested,
//! on any host.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The installed app's bundled ffmpeg, relative to `%LOCALAPPDATA%`. The
/// NSIS installer's per-user default, and where `daemon::ffmpeg()` looks.
const INSTALLED: &str = r"ninja-recorder\libobs\ffmpeg.exe";

pub struct Ffmpeg {
    pub path: PathBuf,
    /// Where the path came from, for the report.
    pub source: &'static str,
    pub version: String,
}

/// Find an ffmpeg that runs, or say where it looked.
pub fn find(explicit: Option<&Path>) -> Result<Ffmpeg, String> {
    let mut candidates: Vec<(PathBuf, &'static str)> = Vec::new();
    if let Some(path) = explicit {
        candidates.push((path.to_path_buf(), "--ffmpeg"));
    } else {
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            candidates.push((
                Path::new(&local).join(INSTALLED),
                "the installed app's bundled copy",
            ));
        }
        candidates.push((PathBuf::from("ffmpeg"), "PATH"));
    }

    let mut tried = Vec::new();
    for (path, source) in candidates {
        match Command::new(&path).arg("-version").output() {
            Ok(out) if out.status.success() => {
                let version = String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .next()
                    .unwrap_or("(no version line)")
                    .to_string();
                return Ok(Ffmpeg {
                    path,
                    source,
                    version,
                });
            }
            Ok(out) => tried.push(format!(
                "{} ({source}): exited {}",
                path.display(),
                out.status
            )),
            Err(e) => tried.push(format!("{} ({source}): {e}", path.display())),
        }
    }
    Err(format!(
        "no runnable ffmpeg. Tried:\n  {}\nInstall the app (its bundled ffmpeg.exe is found automatically), \
         put ffmpeg on PATH, or pass --ffmpeg <path>.",
        tried.join("\n  ")
    ))
}

/// The last values a `-progress` stream reported.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Progress {
    pub frame: Option<u64>,
    /// `out_time_us`: the output timestamp reached, which for a `-f null`
    /// decode is how far into the stream the decoder got.
    pub out_time_us: Option<i64>,
    pub ended: bool,
}

pub fn parse_progress(text: &str) -> Progress {
    let mut p = Progress::default();
    for line in text.lines() {
        let Some((key, value)) = line.trim().split_once('=') else {
            continue;
        };
        match key {
            "frame" => p.frame = value.trim().parse().ok().or(p.frame),
            // `out_time_ms` is also microseconds, for historical reasons; take
            // the correctly named one when both are there.
            "out_time_us" => p.out_time_us = value.trim().parse().ok().or(p.out_time_us),
            "out_time_ms" if p.out_time_us.is_none() => p.out_time_us = value.trim().parse().ok(),
            "progress" => p.ended = value.trim() == "end",
            _ => {}
        }
    }
    p
}

/// `Duration: 00:05:01.23,` from ffmpeg's input banner, in seconds.
pub fn parse_duration(banner: &str) -> Option<f64> {
    let rest = banner.split("Duration: ").nth(1)?;
    let stamp = rest.split(',').next()?.trim();
    let mut parts = stamp.split(':');
    let h: f64 = parts.next()?.parse().ok()?;
    let m: f64 = parts.next()?.parse().ok()?;
    let s: f64 = parts.next()?.parse().ok()?;
    Some(h * 3600.0 + m * 60.0 + s)
}

/// The banner's `Stream #0:n ...` lines, which say what the container holds.
pub fn stream_lines(banner: &str) -> Vec<String> {
    banner
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with("Stream #"))
        .map(str::to_string)
        .collect()
}

/// Lines ffmpeg printed at `-v error`: each is a decode or demux complaint.
pub fn error_lines(stderr: &str) -> Vec<String> {
    stderr
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

pub struct Decode {
    pub ok: bool,
    pub progress: Progress,
    pub errors: Vec<String>,
}

impl Ffmpeg {
    fn run(&self, args: &[&str]) -> Result<(bool, String, String), String> {
        let out = Command::new(&self.path)
            .args(args)
            .output()
            .map_err(|e| format!("could not run {}: {e}", self.path.display()))?;
        Ok((
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        ))
    }

    /// The input banner: container duration and stream list. ffmpeg exits
    /// non-zero here by design (no output was named), so only stderr counts.
    pub fn banner(&self, file: &Path) -> Result<String, String> {
        let file = file.to_string_lossy();
        let (_, _, stderr) = self.run(&["-hide_banner", "-nostdin", "-i", &file])?;
        Ok(stderr)
    }

    /// Decode one stream to nothing. `map` is `0:v:0` or `0:a:0`;
    /// `keyframes_only` skips everything but IDR frames, which makes the frame
    /// count a keyframe count.
    pub fn decode(&self, file: &Path, map: &str, keyframes_only: bool) -> Result<Decode, String> {
        let file = file.to_string_lossy();
        let mut args = vec![
            "-hide_banner",
            "-nostdin",
            "-v",
            "error",
            "-nostats",
            "-progress",
            "pipe:1",
        ];
        if keyframes_only {
            args.extend(["-skip_frame", "nokey"]);
        }
        args.extend(["-i", &file, "-map", map, "-f", "null", "-"]);
        let (ok, stdout, stderr) = self.run(&args)?;
        Ok(Decode {
            ok,
            progress: parse_progress(&stdout),
            errors: error_lines(&stderr),
        })
    }

    /// The app's own finalize step, `remux.rs`'s faststart pass: stream copy
    /// every track into an ordinary MP4 with the index up front. If this
    /// works on a killed file, the daemon can recover one the same way it
    /// finishes a clean one.
    pub fn remux(&self, file: &Path, out: &Path) -> Result<(bool, Vec<String>), String> {
        let (input, output) = (file.to_string_lossy(), out.to_string_lossy());
        let (ok, _, stderr) = self.run(&[
            "-hide_banner",
            "-nostdin",
            "-v",
            "error",
            "-y",
            "-i",
            &input,
            "-map",
            "0",
            "-c",
            "copy",
            "-movflags",
            "+faststart",
            &output,
        ])?;
        Ok((ok, error_lines(&stderr)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROGRESS: &str = "frame=100\nfps=0.0\nout_time_us=1666667\nout_time_ms=1666667\n\
                            progress=continue\nframe=18000\nout_time_us=300000000\n\
                            out_time_ms=300000000\nprogress=end\n";

    #[test]
    fn progress_keeps_the_last_values() {
        let p = parse_progress(PROGRESS);
        assert_eq!(p.frame, Some(18_000));
        assert_eq!(p.out_time_us, Some(300_000_000));
        assert!(p.ended);
    }

    #[test]
    fn progress_tolerates_na_and_the_old_key() {
        let p = parse_progress("frame=0\nout_time_us=N/A\nprogress=end\n");
        assert_eq!(p.frame, Some(0));
        assert_eq!(p.out_time_us, None);
        let p = parse_progress("out_time_ms=5000\n");
        assert_eq!(p.out_time_us, Some(5_000));
    }

    #[test]
    fn duration_and_streams_come_from_the_banner() {
        let banner = "Input #0, mov,mp4,m4a,3gp,3g2,mj2, from 'x.mp4':\n  Metadata:\n    major_brand     : iso6\n  \
                      Duration: 00:05:01.23, start: 0.000000, bitrate: 8123 kb/s\n  \
                      Stream #0:0[0x1](und): Video: h264 (High) (avc1 / 0x31637661), yuv420p, 1920x1080, 60 fps\n  \
                      Stream #0:1[0x2](und): Audio: aac (LC) (mp4a / 0x6134706D), 48000 Hz, stereo, fltp\n\
                      At least one output file must be specified\n";
        let d = parse_duration(banner).unwrap();
        assert!((d - 301.23).abs() < 1e-9);
        let streams = stream_lines(banner);
        assert_eq!(streams.len(), 2);
        assert!(streams[1].contains("Audio: aac"));
        assert_eq!(parse_duration("Duration: N/A, bitrate: N/A"), None);
    }
}
