//! The faststart remux: a stream copy that moves an MP4's seek index to the
//! front, so a fragmented recording can be scrubbed.
//!
//! Every recording is written fragmented (`frag_keyframe+empty_moov`), which
//! is what makes a killed one playable at all: no finalize step, so a crash
//! mid-game still leaves a file. The cost is no upfront seek index, and most
//! players, the review UI's WebView2 `<video>` among them, cannot scrub such a
//! file reliably. A `-c copy` remux with `+faststart` rewrites the container
//! and touches no media, so it is lossless and quick.
//!
//! Two callers, which is why this is not in a backend: the libobs backend's
//! `stop`, on every clean stop, and `db::reconcile::recover_unfinished`, on a
//! recording a dead daemon never got to stop (#233). The argument list is a
//! pure function so the part that has been got wrong before, the stream
//! mapping, is testable without an ffmpeg.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Where the remux writes before it replaces the original.
pub fn tmp_path(video_path: &Path) -> PathBuf {
    video_path.with_extension("faststart.tmp.mp4")
}

/// ffmpeg's arguments for remuxing `input` into `tmp`, for a file with
/// `audio_tracks` audio tracks.
pub fn faststart_args(input: &Path, tmp: &Path, audio_tracks: usize) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec![
        // Overwrite `tmp` without prompting if it is left over from a
        // previous crash.
        "-y".into(),
        "-i".into(),
        input.into(),
    ];
    // Without these, ffmpeg's *default* stream selection applies: one video
    // and one audio stream, the "best" of each. On a multi-track recording
    // that silently drops every stem, and the rename in `remux_faststart`
    // makes it permanent. `0:a?` rather than `0` also skips any data/unknown
    // stream without needing `-ignore_unknown`, and the `?` keeps an
    // audio-less file from failing outright.
    for arg in ["-map", "0:v?", "-map", "0:a?", "-c", "copy", "-movflags", "+faststart"] {
        args.push(arg.into());
    }
    // obs-ffmpeg-mux never sets AV_DISPOSITION_DEFAULT, so without this it is
    // ambiguous which track a player picks. Track 0 is the combined mix and
    // must be the one that plays by default.
    for track in 0..audio_tracks.max(1) {
        args.push(format!("-disposition:a:{track}").into());
        args.push(if track == 0 { "default" } else { "0" }.into());
    }
    args.push(tmp.into());
    args
}

/// Stream-copies `video_path` through ffmpeg with `-movflags +faststart` so
/// the `moov` (seek index) ends up at the front of the file instead of
/// wherever the fragmented writer left it, then atomically replaces the
/// original. On failure the original is left exactly as it was.
pub fn remux_faststart(
    ffmpeg_path: &Path,
    video_path: &Path,
    audio_tracks: usize,
) -> Result<(), String> {
    let tmp = tmp_path(video_path);

    let output = crate::ffmpeg_command(ffmpeg_path)
        .args(faststart_args(video_path, &tmp, audio_tracks))
        .output()
        .map_err(|e| format!("failed to launch ffmpeg at {}: {e}", ffmpeg_path.display()))?;

    if !output.status.success() {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!(
            "ffmpeg exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    std::fs::rename(&tmp, video_path)
        .map_err(|e| format!("failed to replace original file with remuxed one: {e}"))
}

/// Real media files for tests, made by a local ffmpeg.
///
/// Every helper returns `None` when there is no ffmpeg on `PATH`, or one
/// without libx264 and aac, and a test that gets `None` returns early: CI
/// is not guaranteed an ffmpeg, and the pure tests beside each of these
/// cover the decisions without one.
#[cfg(test)]
pub(crate) mod fixtures {
    use std::path::{Path, PathBuf};

    /// The first `ffmpeg` on `PATH`.
    pub(crate) fn ffmpeg() -> Option<PathBuf> {
        let name = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };
        let path = std::env::var_os("PATH")?;
        std::env::split_paths(&path).map(|dir| dir.join(name)).find(|p| p.is_file())
    }

    /// A fresh directory for one test's files.
    pub(crate) fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("ninja-recorder-media-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A three-second fragmented recording, written the way the libobs fork
    /// writes one (`frag_keyframe+empty_moov`): H.264 with a keyframe every
    /// half second, so there are several fragments, and `audio_tracks` AAC
    /// tracks.
    pub(crate) fn fragmented(ffmpeg: &Path, out: &Path, audio_tracks: usize) -> Option<()> {
        let mut command = crate::ffmpeg_command(ffmpeg);
        command.args(["-hide_banner", "-loglevel", "error", "-y"]);
        command.args(["-f", "lavfi", "-i", "testsrc=size=64x64:rate=30"]);
        for track in 0..audio_tracks {
            let tone = format!("sine=frequency={}:sample_rate=48000", 220 * (track + 1));
            command.args(["-f", "lavfi", "-i", &tone]);
        }
        command.args(["-t", "3", "-map", "0:v"]);
        for track in 0..audio_tracks {
            command.args(["-map", &format!("{}:a", track + 1)]);
        }
        command
            .args(["-c:v", "libx264", "-g", "15", "-c:a", "aac"])
            .args(["-movflags", "frag_keyframe+empty_moov"])
            .arg(out);
        let status = command.status().ok()?;
        if !status.success() {
            eprintln!("skipping: this ffmpeg could not write an H.264/AAC fixture");
            return None;
        }
        Some(())
    }

    /// The first `len` bytes of `src`, as a kill would leave them.
    pub(crate) fn truncated_copy(src: &Path, dst: &Path, len: u64) {
        let bytes = std::fs::read(src).unwrap();
        std::fs::write(dst, &bytes[..len as usize]).unwrap();
    }

    pub(crate) fn summary(path: &Path) -> crate::mp4::Summary {
        crate::mp4::summarize(&mut std::fs::File::open(path).unwrap()).unwrap()
    }

    /// `(kind, offset, size)` of every top-level box, so a test can cut a
    /// file inside a particular one. Assumes a whole file with 32-bit sizes,
    /// which is what `fragmented` writes.
    pub(crate) fn boxes(path: &Path) -> Vec<(String, u64, u64)> {
        let bytes = std::fs::read(path).unwrap();
        let mut out = Vec::new();
        let mut at = 0usize;
        while at + 8 <= bytes.len() {
            let size = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
            let kind = String::from_utf8_lossy(&bytes[at + 4..at + 8]).into_owned();
            assert!(size >= 8, "a fixture box with a size of {size}");
            out.push((kind, at as u64, size as u64));
            at += size;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(args: &[OsString]) -> Vec<String> {
        args.iter().map(|a| a.to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn one_track_maps_everything_and_marks_it_default() {
        let args = faststart_args(Path::new("in.mp4"), Path::new("out.tmp.mp4"), 1);
        assert_eq!(
            strings(&args),
            [
                "-y", "-i", "in.mp4", "-map", "0:v?", "-map", "0:a?", "-c", "copy",
                "-movflags", "+faststart", "-disposition:a:0", "default", "out.tmp.mp4",
            ]
        );
    }

    /// The multi-track case: every stem is mapped, only the mix is default.
    #[test]
    fn four_tracks_keep_every_stem_and_only_the_mix_is_default() {
        let args = strings(&faststart_args(Path::new("in.mp4"), Path::new("t.mp4"), 4));
        let tail: Vec<&str> = args[11..].iter().map(String::as_str).collect();
        assert_eq!(
            tail,
            [
                "-disposition:a:0", "default",
                "-disposition:a:1", "0",
                "-disposition:a:2", "0",
                "-disposition:a:3", "0",
                "t.mp4",
            ]
        );
        assert!(args.windows(2).any(|w| w == ["-map", "0:a?"]));
        assert!(!args.iter().any(|a| a == "-c:v" || a == "-c:a"), "a stream copy only");
    }

    #[test]
    fn the_tmp_file_sits_beside_the_recording() {
        assert_eq!(
            tmp_path(Path::new("/r/game.mp4")),
            PathBuf::from("/r/game.faststart.tmp.mp4")
        );
    }

    /// End to end against a real ffmpeg: a fragmented two-track file comes out
    /// moov-first, with both audio tracks still there.
    #[test]
    fn a_fragmented_file_comes_out_moov_first_with_every_track() {
        let Some(ffmpeg) = fixtures::ffmpeg() else {
            eprintln!("skipping: no ffmpeg on PATH");
            return;
        };
        let dir = fixtures::dir("remux");
        let file = dir.join("game.mp4");
        if fixtures::fragmented(&ffmpeg, &file, 2).is_none() {
            return;
        }
        let before = fixtures::summary(&file);
        assert!(before.mvex && before.structurally_playable());

        remux_faststart(&ffmpeg, &file, 2).unwrap();

        let after = fixtures::summary(&file);
        assert!(!after.mvex, "no longer fragmented");
        assert!(after.moov_offset < after.first_mdat_offset, "the index is up front");
        assert_eq!(after.audio_tracks, 2);
        assert!(!tmp_path(&file).exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_failed_remux_leaves_the_original_alone() {
        let Some(ffmpeg) = fixtures::ffmpeg() else {
            eprintln!("skipping: no ffmpeg on PATH");
            return;
        };
        let dir = fixtures::dir("remux-fail");
        let file = dir.join("not-a-video.mp4");
        std::fs::write(&file, b"not a video").unwrap();

        assert!(remux_faststart(&ffmpeg, &file, 1).is_err());
        assert_eq!(std::fs::read(&file).unwrap(), b"not a video");
        assert!(!tmp_path(&file).exists());
        std::fs::remove_dir_all(&dir).ok();
    }
}
