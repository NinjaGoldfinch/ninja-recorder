//! Audio-stem sidecar files for the review player.
//!
//! The review player can't ask WebView2 to pick a different audio track out
//! of a multi-track mp4 — `HTMLMediaElement.audioTracks` is behind an
//! experimental Blink flag on a runtime whose version we don't control. So
//! selecting a stem extracts it to its own small file, which a hidden
//! `<audio>` plays in lockstep with the muted video. Track 0 is the combined
//! mix and never comes through here; see DEVELOPMENT.md §2.5.
//!
//! Sidecars live in a subdirectory of the recordings folder rather than
//! beside the mp4s. That is deliberate and load-bearing: `db::reconcile`
//! scans the recordings folder non-recursively and imports every video file
//! it finds, so a sidecar sitting next to its recording would show up in the
//! library as a phantom VOD.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Subdirectory of the recordings folder holding extracted stems. Also
/// listed in `tauri.conf.json`'s `assetProtocol.scope` — the webview can't
/// load a file the scope doesn't cover, and the existing `recordings/*`
/// entry doesn't match across a `/`.
pub const CACHE_DIR: &str = "audio-tracks";

/// Where the stem for `track_index` of `recording_path` lives, extracted or
/// not. `.mp4`, not `.m4a`: Tauri's asset protocol sniffs the container to
/// pick a Content-Type, and an `M4A ` brand comes back as the non-standard
/// `audio/m4a` which the webview may refuse. An `isom`-branded `.mp4` is
/// served as `video/mp4` and plays.
pub fn sidecar_path(recordings_dir: &Path, recording_path: &Path, track_index: usize) -> PathBuf {
    let stem = recording_path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "recording".to_string());
    recordings_dir
        .join(CACHE_DIR)
        .join(format!("{stem}.a{track_index}.mp4"))
}

/// Extracts one audio track, reusing an existing sidecar if there is one.
///
/// `-c copy` on a single audio stream, so this rewrites tens of megabytes
/// rather than re-encoding the multi-gigabyte video.
pub fn extract(
    ffmpeg_path: &Path,
    recordings_dir: &Path,
    recording_path: &Path,
    track_index: usize,
) -> Result<PathBuf, String> {
    let dest = sidecar_path(recordings_dir, recording_path, track_index);
    if dest.exists() {
        return Ok(dest);
    }
    if !recording_path.exists() {
        return Err(format!("{} no longer exists", recording_path.display()));
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    let output = Command::new(ffmpeg_path)
        .arg("-y")
        .arg("-i")
        .arg(recording_path)
        // `0:a:N` indexes the Nth *audio* stream, which is the track index
        // the layout in the DB describes — ffmpeg preserves stream order
        // through a stream copy.
        .args(["-map", &format!("0:a:{track_index}")])
        .args(["-c", "copy", "-movflags", "+faststart"])
        // Forced rather than inferred from the extension, so the output
        // carries an `isom` brand. See `sidecar_path`.
        .args(["-f", "mp4"])
        .arg(&dest)
        .output()
        .map_err(|e| format!("failed to launch ffmpeg at {}: {e}", ffmpeg_path.display()))?;

    if !output.status.success() {
        // A partial file would be treated as a cache hit next time.
        let _ = std::fs::remove_file(&dest);
        return Err(format!(
            "ffmpeg could not extract audio track {track_index}: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(dest)
}

/// Removes every stem extracted from `recording_path`.
///
/// Called when a recording is deleted, by the user or by retention. Without
/// it the sidecars outlive their recording forever: nothing else knows they
/// exist, and they aren't counted in the library's disk usage.
pub fn remove_for(recordings_dir: &Path, recording_path: &Path) {
    let Some(stem) = recording_path.file_stem().map(|s| s.to_string_lossy().into_owned()) else {
        return;
    };
    let cache = recordings_dir.join(CACHE_DIR);
    let Ok(entries) = std::fs::read_dir(&cache) else {
        return; // never extracted anything for any recording yet
    };
    let prefix = format!("{stem}.a");
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(&prefix) && name.ends_with(".mp4") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidecar_lives_in_the_cache_dir_not_beside_the_recording() {
        let path = sidecar_path(
            Path::new("/vods"),
            Path::new("/vods/recording-123.mp4"),
            2,
        );
        assert_eq!(path, Path::new("/vods/audio-tracks/recording-123.a2.mp4"));
        // The reason: reconcile scans /vods non-recursively and would
        // import anything video-shaped it found there.
        assert_ne!(path.parent(), Some(Path::new("/vods")));
    }

    #[test]
    fn sidecars_are_mp4_so_the_asset_protocol_serves_them_as_video_mp4() {
        let path = sidecar_path(Path::new("/vods"), Path::new("/vods/a.mp4"), 1);
        assert_eq!(path.extension().unwrap(), "mp4");
    }

    #[test]
    fn remove_for_deletes_only_that_recordings_stems() {
        let dir = std::env::temp_dir().join(format!("nr-stems-{}", std::process::id()));
        let cache = dir.join(CACHE_DIR);
        std::fs::create_dir_all(&cache).unwrap();

        let keep = cache.join("other.a1.mp4");
        for name in ["target.a1.mp4", "target.a2.mp4"] {
            std::fs::write(cache.join(name), b"x").unwrap();
        }
        std::fs::write(&keep, b"x").unwrap();

        remove_for(&dir, Path::new("/anywhere/target.mp4"));

        assert!(!cache.join("target.a1.mp4").exists());
        assert!(!cache.join("target.a2.mp4").exists());
        assert!(keep.exists(), "another recording's stems must survive");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn remove_for_is_quiet_when_nothing_was_ever_extracted() {
        remove_for(
            Path::new("/definitely/not/a/real/dir"),
            Path::new("/x/recording.mp4"),
        );
    }
}
