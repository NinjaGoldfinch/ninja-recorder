//! Container duration, read back out of a video file with ffmpeg.
//!
//! Recordings this app made get their length from the session clock at
//! finalize. Files `db::reconcile` imported from the folder had no session
//! — it knows only the path, the size and the mtime — so the only place
//! their length can come from is the container itself.
//!
//! **ffmpeg, not ffprobe.** Only `ffmpeg.exe` is staged into the bundle
//! (`.github/workflows/ci.yml`, "Stage ffmpeg for faststart remux"), so
//! ffprobe's clean `-show_format` JSON is not available to us and adding it
//! would double the download for one number. `ffmpeg -i` with no output file
//! prints the same container header to stderr and exits non-zero saying "At
//! least one output file must be specified" — so the exit status is ignored
//! and stderr is what gets read.
//!
//! That leaves us parsing prose, which is the thing this repo generally
//! avoids. It is tolerable here only because the line is one of ffmpeg's
//! oldest and most stable outputs, and because getting it wrong costs a NULL
//! column rather than a wrong recording: every failure path returns `None`.

use std::path::Path;

/// The container's duration in seconds, or `None` if it can't be read.
///
/// Never an error: a missing ffmpeg, an unreadable file, a file still being
/// written, or wording this parser doesn't recognize all mean the column
/// stays NULL. `reconcile` must not start failing over a cosmetic value —
/// see the module header.
pub fn duration_s(ffmpeg: &Path, video: &Path) -> Option<f64> {
    let output = crate::ffmpeg_command(ffmpeg)
        .arg("-hide_banner")
        .arg("-i")
        .arg(video)
        // ffmpeg reads stdin for interactive keys and would sit there
        // forever if it ever decided to prompt. Nothing here answers it.
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;

    // Deliberately not checked: with no output file ffmpeg always exits 1,
    // having already printed everything we came for.
    parse_duration_s(&String::from_utf8_lossy(&output.stderr))
}

/// Pulls the seconds out of ffmpeg's `Duration:` line.
///
/// Split out from the spawn so the wording — the part that can actually be
/// got wrong — is directly testable without an ffmpeg on the box. The line
/// looks like:
///
/// ```text
///   Duration: 00:31:20.53, start: 0.000000, bitrate: 8123 kb/s
/// ```
fn parse_duration_s(stderr: &str) -> Option<f64> {
    // First occurrence: we pass one input, so the first `Duration:` is the
    // container's. Later ones would come from chapter or metadata dumps.
    // Scoped to its own line before the comma split, so a format that
    // prints no `, start:` after it doesn't swallow the stream list.
    let line = stderr.lines().find(|line| line.contains("Duration:"))?;
    let (_, rest) = line.split_once("Duration:")?;
    // Up to the `start:` that normally follows on the same line. `N/A` —
    // what a stream, or a file still being written, reports — has no colons
    // and falls out of the parse below on its own.
    let field = rest.split(',').next()?.trim();

    let mut parts = field.split(':');
    let hours: f64 = parts.next()?.parse().ok()?;
    let minutes: f64 = parts.next()?.parse().ok()?;
    let seconds: f64 = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None; // not the shape we think it is; don't guess
    }

    let total = hours * 3600.0 + minutes * 60.0 + seconds;
    // A zero-length container is a truncated or still-growing file, not a
    // recording of no length. NULL says "unknown", which is the truth;
    // 0 would render as `0:00` and read as a fact.
    (total > 0.0).then_some(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape this parser exists to read.
    ///
    /// **Written by hand, not captured.** No box in this project's dev loop
    /// has an ffmpeg on it, so this is ffmpeg's documented output format
    /// rather than a recorded one — which is exactly why
    /// `docs/windows-verification.md` carries an item to confirm the real
    /// wording against the bundled binary.
    const REAL_OUTPUT: &str = "\
Input #0, mov,mp4,m4a,3gp,3g2,mj2, from 'recording-1788678369340.mp4':
  Metadata:
    major_brand     : isom
    minor_version   : 512
    compatible_brands: isomiso2avc1mp41
    encoder         : Lavf61.7.100
  Duration: 00:31:20.53, start: 0.000000, bitrate: 8123 kb/s
  Stream #0:0[0x1](und): Video: h264 (High) (avc1 / 0x31637661), yuv420p(tv, bt709, progressive), 1920x1080 [SAR 1:1 DAR 16:9], 7985 kb/s, 60 fps, 60 tbr, 15360 tbn (default)
  Stream #0:1[0x2](und): Audio: aac (LC) (mp4a / 0x6134706D), 48000 Hz, stereo, fltp, 128 kb/s (default)
At least one output file must be specified
";

    #[test]
    fn reads_the_duration_line() {
        let seconds = parse_duration_s(REAL_OUTPUT).unwrap();
        assert!((seconds - 1880.53).abs() < 0.001, "got {seconds}");
    }

    #[test]
    fn na_duration_is_unknown() {
        // What a file still being written commonly reports, and the case
        // the issue asks to survive rather than guess at.
        let stderr = "  Duration: N/A, start: 0.000000, bitrate: N/A\n";
        assert_eq!(parse_duration_s(stderr), None);
    }

    #[test]
    fn zero_duration_is_unknown_not_zero() {
        assert_eq!(parse_duration_s("  Duration: 00:00:00.00, start: 0.0\n"), None);
    }

    #[test]
    fn output_without_a_duration_line_is_unknown() {
        let stderr = "recording.mp4: No such file or directory\n";
        assert_eq!(parse_duration_s(stderr), None);
    }

    #[test]
    fn takes_the_first_duration_when_more_than_one_appears() {
        let stderr = "  Duration: 00:00:10.00, start: 0.0\n  Duration: 00:99:99.00, start: 0.0\n";
        assert_eq!(parse_duration_s(stderr), Some(10.0));
    }

    #[test]
    fn hours_and_fractional_seconds_both_count() {
        let seconds = parse_duration_s("  Duration: 02:03:04.50, start: 0.0\n").unwrap();
        assert!((seconds - 7384.5).abs() < 0.001, "got {seconds}");
    }

    #[test]
    fn an_unexpected_shape_is_unknown_rather_than_a_guess() {
        assert_eq!(parse_duration_s("  Duration: 1:2:3:4, start: 0.0\n"), None);
    }

    #[test]
    fn a_missing_ffmpeg_binary_is_unknown_rather_than_an_error() {
        let missing = Path::new("/definitely/not/an/ffmpeg/binary");
        assert_eq!(duration_s(missing, Path::new("/tmp/whatever.mp4")), None);
    }
}
