//! The whole of the daemon's observable state, in one message. WS2 task 2.4.
//!
//! A client that connects, or reconnects after a dropped pipe, gets one
//! `Snapshot` and then a stream of events. It never replays history. That is
//! what makes a UI killed mid-game correct the moment it comes back: there is
//! no backlog to drain and no way to be subtly behind, because the first thing
//! across the wire is the answer to every question the UI can ask, and
//! everything after it is a delta (implementation plan §4.2, §4.3).
//!
//! ## Why the fields are these fields
//!
//! Each one is state the UI would otherwise have to fetch with a command at
//! startup, and each has an event that keeps it current afterwards. The
//! snapshot and the event stream describe the same surface, so a field here
//! with no event would go stale silently, and an event with no field here would
//! leave a reconnecting client unable to establish a baseline for it.
//!
//! | Field | Kept current by |
//! |---|---|
//! | `state` | `Event::StateChanged` |
//! | `lcu` | `Event::LcuPhase` |
//! | `current_recording` | `Event::MarkerAdded`, `SampleBatch` |
//! | `update` | `Event::UpdateStatus` |
//! | `prefs` | no event today; the UI writes these and owns the echo |
//!
//! `prefs` is the deliberate exception and worth naming. Preferences change
//! only because a UI changed them, so the writer already knows; carrying them
//! here is about the *other* window, and about a UI that restarted, rather than
//! about staying in step.
//!
//! ## `seq` is supplied, not derived
//!
//! The counter belongs to the event stream, and the event stream belongs to the
//! daemon (WS3.1). `Ctx` has none, so assembling a snapshot takes one rather
//! than inventing one. The contract's job is to say that a snapshot *is*
//! positioned in the stream and that a client can compare against it; deciding
//! what the number counts is the transport's.
//!
//! The rule it exists to support: an event numbered at or below `seq` is
//! already reflected here and must be dropped, and the first event a client
//! applies is `seq + 1`. Without it a snapshot assembled while events were in
//! flight would be applied over newer state.
//!
//! ## `lcu` is supplied too, and for a sharper reason
//!
//! `core::lcu_status` is `async`, discovers the lockfile itself, and makes two
//! HTTPS round trips; its own doc calls it a smoke test. Putting that on the
//! path of every client connect would make `hello` cost a network request, and
//! would re-derive something the process already knows, since the state machine
//! keeps the phase live through `lcu::gameflow::watch`.
//!
//! So the caller passes in the last observed status rather than this asking for
//! a fresh one. The cache that makes it cheap belongs to whatever owns the
//! gameflow watcher, which is WS3's to build; declaring the field here does not
//! pre-empt it.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::core::{Ctx, LcuStatus};
use crate::state_machine::machine::GameState;
use crate::update::UpdateStatus;

/// One message carrying everything a fresh client needs to render.
///
/// Nothing constructs this until WS3.1 has a pipe to send it down. Declaring
/// the shape is the point of this task, exactly as `contract::events` declares
/// four variants nothing emits yet; the alternative is a transport inventing
/// its own message shape and a contract that describes something else.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    /// The position in the event stream this snapshot was taken at. See the
    /// module header: events at or below it are already included here.
    pub seq: u64,
    /// What the state machine is doing.
    ///
    /// Deliberately the bare `GameState` and not `SupervisorStatus`. That type
    /// also carries `last_finalized`, which is a whole `FinalizedRecording`
    /// including every marker of the last game. A snapshot is sent on every
    /// connect and reconnect, so a field that grows with the length of a game
    /// is the wrong shape for it, and the library already answers "what did the
    /// last game produce" through `list_recordings` and
    /// `get_recording_markers`. What is live rather than stored is in
    /// `current_recording` below.
    pub state: GameState,
    /// Whether the League client is there, and what it says it is doing.
    pub lcu: LcuStatus,
    /// The recording in flight, when there is one. `None` is the resting
    /// state and is not an error.
    pub current_recording: Option<CurrentRecording>,
    /// The last thing the background update check found.
    pub update: UpdateStatus,
    /// The `settings_kv` rows the frontend treats as its preference cache.
    ///
    /// A map rather than a struct because the table is deliberately
    /// schemaless: a missing key means "use the frontend default", which is
    /// what lets a new preference ship without a migration
    /// ([DEVELOPMENT.md §5.1](../../../DEVELOPMENT.md)).
    pub prefs: HashMap<String, String>,
}

/// The in-flight recording, as against the last finished one.
///
/// Separate from `SupervisorStatus::last_finalized` because they answer
/// different questions and are almost never both interesting: one is "what is
/// happening now", the other is "what did the last game produce".
///
/// The counts are what the session has accumulated so far, not what the row
/// will hold. Markers keep arriving until the game ends, so a client that
/// renders this is rendering a running total and should say so.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct CurrentRecording {
    /// The file stem capture is writing to. A stem rather than a path because
    /// `Recorder::start` returns `Ok(())` and the extension is not settled
    /// until `stop` hands back a `RecordingOutput`.
    pub file_stem: String,
    /// When capture actually started, in epoch milliseconds.
    pub started_at_millis: i64,
    /// Seconds of capture so far. Read from the session rather than timed by
    /// the client, so a window opened part-way through a game does not report
    /// a counter that started when it happened to look.
    pub elapsed_s: f64,
    /// Markers collected so far this game.
    pub marker_count: usize,
    /// Advantage samples collected so far this game.
    pub sample_count: usize,
    /// The game-time to video-time offset in force, or `None` while the game
    /// clock has not been seen to advance.
    ///
    /// `None` is not zero. "Not yet known" and "aligned" are different states,
    /// and reading one as the other is how a marker lands in the wrong place
    /// ([docs/recording-pipeline.md](../../../docs/recording-pipeline.md)).
    pub alignment_offset_s: Option<f64>,
}

impl Snapshot {
    /// Assemble one from everything `Ctx` already knows, plus the two values
    /// it cannot own: the stream position and the last observed LCU status.
    ///
    /// Every read is a lock taken and released, never held across another, so
    /// this cannot deadlock against the supervisor's own work. It is a
    /// consistent-enough view rather than an atomic one: the alternative is one
    /// lock over the whole of `Ctx`, which would put a snapshot on the path of
    /// every marker write for a difference no client can observe.
    ///
    /// `prefs` degrades to empty rather than failing the whole snapshot. A
    /// client that gets no preferences falls back to the frontend defaults,
    /// which is the same thing a fresh install does; a client that gets no
    /// snapshot cannot render at all.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn assemble(ctx: &Ctx, seq: u64, lcu: LcuStatus) -> Self {
        Self {
            seq,
            state: ctx.supervisor.status().state,
            lcu,
            current_recording: ctx.supervisor.current_recording(),
            update: crate::core::get_update_status(ctx).unwrap_or(UpdateStatus::Unsupported),
            prefs: ctx.db.get_ui_prefs().unwrap_or_default(),
        }
    }
}

#[cfg(test)]
mod tests {
    //! The exit criterion is a serde round trip, which is a stronger claim
    //! than it sounds: it fails if any nested type is serialize-only, which
    //! every one of them was before this task.

    use super::*;

    fn a_current_recording() -> CurrentRecording {
        CurrentRecording {
            file_stem: "recording-20260915-084500".to_string(),
            started_at_millis: 1_757_925_900_000,
            elapsed_s: 612.5,
            marker_count: 14,
            sample_count: 612,
            alignment_offset_s: Some(17.7),
        }
    }

    #[test]
    fn current_recording_round_trips() {
        let before = a_current_recording();
        let json = serde_json::to_string(&before).expect("serialize");
        let after: CurrentRecording = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(before, after);
    }

    /// `None` and `0.0` are different states for the alignment, so the round
    /// trip has to preserve the difference rather than collapse it.
    #[test]
    fn an_unproven_alignment_survives_as_none() {
        let before = CurrentRecording {
            alignment_offset_s: None,
            ..a_current_recording()
        };
        let json = serde_json::to_string(&before).expect("serialize");
        assert!(
            json.contains("\"alignmentOffsetS\":null"),
            "an unknown alignment must stay distinguishable from 0.0: {json}"
        );
        let after: CurrentRecording = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(before, after);
    }

    /// The field names on the wire are camelCase, because the TypeScript
    /// client is generated from these declarations and reads them directly.
    #[test]
    fn the_wire_names_are_camel_case() {
        let json = serde_json::to_string(&a_current_recording()).expect("serialize");
        for key in [
            "fileStem",
            "startedAtMillis",
            "elapsedS",
            "markerCount",
            "sampleCount",
            "alignmentOffsetS",
        ] {
            assert!(json.contains(key), "{key} missing from {json}");
        }
        assert!(!json.contains("file_stem"), "snake_case leaked: {json}");
    }
}
