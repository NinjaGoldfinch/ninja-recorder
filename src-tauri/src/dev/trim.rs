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
//!
//! The post-game tail follows the stored diagnostics exactly as the finalize's
//! trim does (`trim::TailEvidence`, #305): a recording from before they said
//! how its polls ended is not known to end in post-game, and keeps its end.

use crate::trim::{trim_recording, TrimReport};

pub fn dev_trim_lead_in(
    ctx: &crate::core::Ctx,
    recording_id: i64,
) -> Result<TrimReport, String> {
    let ffmpeg = ctx
        .ffmpeg
        .as_deref()
        .ok_or_else(|| "no ffmpeg in this build, so nothing can cut a file".to_string())?;
    trim_recording(&ctx.db, ffmpeg, recording_id)
}
