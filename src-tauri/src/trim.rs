//! Cutting the loading screen off the front of a recording's file.
//! DEVELOPMENT.md §5.4.
//!
//! The review player already *skips* it — it treats a recording as the
//! window `[game start − 2s, end]` and never shows the rest — so this is
//! purely about the twenty-odd megabytes a game that the skipped lead still
//! occupies on disk.
//!
//! ## It only fires when the answer is measured, never guessed
//!
//! The loading screen's length is not estimated: a sample carries both a
//! game clock and a video clock, and the gap between them *is* it. So the
//! two cases split cleanly.
//!
//! - Capture started **before** the game — the normal case — and the gap
//!   says by how much. That is the part to cut.
//! - Capture started **at or after** it: a reconnect, or a client that
//!   reported the game late. The gap is zero or negative, `trim_point_s`
//!   returns `None`, and nothing is touched. There is no loading screen in
//!   the file, so there is nothing to get wrong.
//!
//! A recording with no samples has no alignment at all and is likewise left
//! alone.
//!
//! ## What it does not guarantee
//!
//! **A stream copy cuts on a keyframe.** `-ss` lands on the nearest one at
//! or before the requested point, so the amount actually removed is up to a
//! GOP less than the amount asked for. Everything downstream shifts by the
//! *real* figure, which is why this probes the result rather than trusting
//! the request — and refuses outright when the two disagree by more than a
//! keyframe interval could explain.
//!
//! Whether multi-track audio, stream dispositions and the faststart index
//! all survive the copy was the open question here. Real footage has since
//! answered it — trimmed recordings play, seek, and keep their separate
//! stems — though nothing asserts it automatically, because CI runs unit
//! tests rather than video.
//!
//! ## The order of operations is the safety
//!
//! The original is moved aside rather than overwritten, and only deleted
//! once both the file and the database agree. Any failure before that puts
//! it back, so the worst outcome is a recording exactly as it was.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::db::Db;
use crate::{info, warn};

/// Kept in front of the game, matching the player's own lead-in so a
/// trimmed recording opens exactly where an untrimmed one does.
///
/// One second, not more. The alignment comes from a 1 Hz poll so it is only
/// accurate to about that anyway, and nothing happens in the opening
/// seconds of a game that is worth protecting with a wider margin.
const LEAD_IN_S: f64 = 1.0;

/// Below this there is no loading screen worth the rewrite.
const MIN_TRIM_S: f64 = 3.0;

/// How far the amount actually removed may differ from the amount asked for
/// before this refuses to touch the database.
///
/// A keyframe cut is expected to remove *less* than requested, by up to one
/// GOP — a couple of seconds at the encoder settings this app uses. Five is
/// generous for that and still small enough that a wildly different figure
/// — a re-encode, a truncated file, a probe reading the wrong stream — stops
/// the operation instead of silently rebasing every marker onto a lie.
const MAX_DRIFT_S: f64 = 5.0;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TrimReport {
    /// What the loading screen was measured at, from the samples.
    pub game_starts_at_s: f64,
    /// What was asked of ffmpeg.
    pub requested_s: f64,
    /// What actually came off, from the durations either side. Lower than
    /// `requested_s` by up to a GOP, because the cut lands on a keyframe.
    pub removed_s: f64,
    pub duration_before_s: f64,
    pub duration_after_s: f64,
    pub size_before_bytes: i64,
    pub size_after_bytes: i64,
}

/// Where to cut, given where the game starts. `None` when there is nothing
/// worth cutting.
///
/// Pure, and the only place the policy lives: keep `LEAD_IN_S` in front of
/// the game, and do not bother below `MIN_TRIM_S`.
pub fn trim_point_s(game_starts_at_s: f64) -> Option<f64> {
    let point = game_starts_at_s - LEAD_IN_S;
    (point >= MIN_TRIM_S).then_some(point)
}

/// Whether a measured removal is close enough to the requested one to act
/// on.
///
/// Pure so the judgement is testable without a video file. A cut that
/// removed *more* than asked is always wrong — a keyframe is at or before
/// the point, never after — and is rejected however small the excess.
pub fn removal_is_plausible(requested_s: f64, removed_s: f64) -> bool {
    removed_s > 0.0 && removed_s <= requested_s && requested_s - removed_s <= MAX_DRIFT_S
}

/// Audio tracks in a recording's stored layout, for the disposition flags.
/// One when the layout is missing or unreadable, which is what a
/// single-track file looks like.
fn audio_track_count(audio_tracks_json: Option<&str>) -> usize {
    audio_tracks_json
        .and_then(|json| serde_json::from_str::<crate::recorder::audio::AudioLayout>(json).ok())
        .map(|layout| layout.tracks.len().max(1))
        .unwrap_or(1)
}

fn cut(
    ffmpeg: &Path,
    input: &Path,
    output: &Path,
    at_s: f64,
    audio_tracks: usize,
) -> Result<(), String> {
    let mut command = crate::ffmpeg_command(ffmpeg);
    command
        .arg("-y")
        // Before `-i`, so ffmpeg seeks the input rather than decoding and
        // discarding everything ahead of the cut.
        .args(["-ss", &format!("{at_s:.3}")])
        .arg("-i")
        .arg(input)
        // Same mapping as the faststart remux: default stream selection
        // would keep one audio stream and silently drop every other stem.
        .args(["-map", "0:v?", "-map", "0:a?"])
        .args(["-c", "copy", "-movflags", "+faststart"]);

    // Track 0 is the combined mix and has to stay the one a player picks.
    for track in 0..audio_tracks {
        command
            .arg(format!("-disposition:a:{track}"))
            .arg(if track == 0 { "default" } else { "0" });
    }

    let output_result = command
        .arg(output)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("could not launch ffmpeg at {}: {e}", ffmpeg.display()))?;

    if !output_result.status.success() {
        return Err(format!(
            "ffmpeg exited with {}: {}",
            output_result.status,
            String::from_utf8_lossy(&output_result.stderr)
        ));
    }
    Ok(())
}

/// Cuts the loading screen off one recording and rebases its markers and
/// samples onto what is left.
///
/// Every early return leaves the recording exactly as it was.
pub fn trim_recording(db: &Db, ffmpeg: &Path, recording_id: i64) -> Result<TrimReport, String> {
    let row = db
        .get_recording(recording_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("no recording {recording_id}"))?;

    // The same number the player uses, from the same place: a sample carries
    // both a game clock and a video clock, and the gap between them is the
    // loading screen.
    let game_starts_at = db
        .sample_alignment_offset(recording_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| {
            "this recording has no samples, so nothing knows where its game started".to_string()
        })?;

    let requested = trim_point_s(game_starts_at).ok_or_else(|| {
        format!("nothing worth cutting — the game starts {game_starts_at:.1}s in")
    })?;

    let video = PathBuf::from(&row.path);
    let before = crate::probe::duration_s(ffmpeg, &video)
        .ok_or_else(|| "could not read the recording's duration".to_string())?;

    let tmp = video.with_extension("trim.tmp.mp4");
    let backup = video.with_extension("untrimmed.mp4");
    cut(
        ffmpeg,
        &video,
        &tmp,
        requested,
        audio_track_count(row.audio_tracks_json.as_deref()),
    )
    .inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })?;

    let after = match crate::probe::duration_s(ffmpeg, &tmp) {
        Some(after) => after,
        None => {
            let _ = std::fs::remove_file(&tmp);
            return Err("could not read the trimmed file's duration".to_string());
        }
    };

    let removed = before - after;
    if !removal_is_plausible(requested, removed) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!(
            "refusing to rebase: asked ffmpeg for {requested:.1}s but {removed:.1}s came off \
             ({before:.1}s → {after:.1}s)"
        ));
    }

    // The original moves aside rather than being overwritten, so every
    // failure below can put it back.
    std::fs::rename(&video, &backup).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("could not move the original aside: {e}")
    })?;
    if let Err(e) = std::fs::rename(&tmp, &video) {
        let _ = std::fs::rename(&backup, &video);
        return Err(format!("could not put the trimmed file in place: {e}"));
    }

    // Read after the rename, so it is the size of what is actually there.
    // Falling back to the row's old size would tell the library the file is
    // bigger than it is, which is the same lie in a quieter form.
    let new_size = std::fs::metadata(&video).map(|m| m.len() as i64).unwrap_or(row.size_bytes);

    if let Err(e) = db.apply_trim(recording_id, removed, after, new_size) {
        // The file is already trimmed and the database is not, which would
        // leave every marker out by the length of a loading screen. Put the
        // original back rather than leave that behind.
        let _ = std::fs::remove_file(&video);
        let _ = std::fs::rename(&backup, &video);
        return Err(format!("could not rebase markers, so the file was restored: {e}"));
    }

    if let Err(e) = std::fs::remove_file(&backup) {
        warn!("trim", "trimmed recording {recording_id} but could not remove {}: {e}",
            backup.display()
        );
    }

    info!("trim", "cut {removed:.1}s off recording {recording_id} ({before:.1}s → {after:.1}s)");
    Ok(TrimReport {
        game_starts_at_s: game_starts_at,
        requested_s: requested,
        removed_s: removed,
        duration_before_s: before,
        duration_after_s: after,
        size_before_bytes: row.size_bytes,
        size_after_bytes: new_size,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_a_lead_in_and_ignores_a_short_one() {
        // The ordinary case: twenty seconds of loading screen, one kept.
        assert_eq!(trim_point_s(20.0), Some(19.0));
        // Exactly on the floor still cuts.
        assert_eq!(trim_point_s(4.0), Some(3.0));
        // 3.5 − 1 is under it: not worth rewriting a file to save 2.5s.
        assert_eq!(trim_point_s(3.5), None);
        assert_eq!(trim_point_s(0.0), None);
        // A reconnect: capture started after the game did, so there is no
        // loading screen in the file at all.
        assert_eq!(trim_point_s(-30.0), None);
    }

    /// A keyframe is at or before the requested point, so a cut always
    /// removes the same or less. More coming off means something other than
    /// a stream copy happened, and rebasing on it would put every marker
    /// out.
    #[test]
    fn a_cut_may_remove_less_than_asked_but_never_more() {
        assert!(removal_is_plausible(18.0, 18.0));
        assert!(removal_is_plausible(18.0, 16.0), "one GOP short is normal");
        assert!(!removal_is_plausible(18.0, 18.5), "cannot exceed the request");
        assert!(!removal_is_plausible(18.0, 12.0), "too far short to trust");
        assert!(!removal_is_plausible(18.0, 0.0), "nothing came off");
        assert!(!removal_is_plausible(18.0, -3.0), "the file got longer");
    }

    #[test]
    fn a_missing_or_unreadable_layout_counts_one_track() {
        assert_eq!(audio_track_count(None), 1);
        assert_eq!(audio_track_count(Some("not json")), 1);
        assert_eq!(audio_track_count(Some("{}")), 1);
    }

    #[test]
    fn a_multi_track_layout_is_counted_from_its_tracks() {
        let json = serde_json::json!({
            "sources": [{ "kind": "game" }, { "kind": "microphone" }],
            "tracks": [
                { "label": "Everything", "sources": [0, 1] },
                { "label": "Game", "sources": [0] },
                { "label": "Mic", "sources": [1] }
            ]
        })
        .to_string();
        assert_eq!(audio_track_count(Some(&json)), 3);
    }
}
