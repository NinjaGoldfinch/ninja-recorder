//! Cutting the loading screen off a recording that already has one.
//!
//! Finalize does this on its own now (`crate::trim`). This is for the
//! recordings made before it did, and for re-running one by hand when a
//! trim was skipped — a session with no ffmpeg, or a file still being
//! written when the finalize reached for it.
//!
//! It is a no-op on anything already trimmed: the rebase moved the samples
//! with the file, so the measured loading screen is then under the floor and
//! `trim_point_s` returns `None`.

use crate::trim::{trim_recording, TrimReport};
use crate::AppState;

#[tauri::command]
pub fn dev_trim_lead_in(
    state: tauri::State<AppState>,
    recording_id: i64,
) -> Result<TrimReport, String> {
    let ffmpeg = state
        .ffmpeg
        .as_deref()
        .ok_or_else(|| "no ffmpeg in this build, so nothing can cut a file".to_string())?;
    trim_recording(&state.db, ffmpeg, recording_id)
}
