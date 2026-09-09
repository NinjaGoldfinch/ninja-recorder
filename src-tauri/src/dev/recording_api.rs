//! One recording's whole story, from every source that has one.
//!
//! The information already exists; it is just scattered. The Database panel
//! shows the raw row, Diagnostics shows `diagnostics_json` for the 25 most
//! recent, the Log shows what the poller saw, and the review view shows the
//! markers. Nothing put a single recording's sources side by side, which is
//! exactly what is wanted when a row looks wrong — a role that says Jungle
//! for a top game, empty item slots, a missing champion (#99).
//!
//! **Provenance is the point, not the columns.** A value is much easier to
//! doubt when the thing that wrote it is named beside it: `champion` and
//! `role` each have two possible writers and a rule about which wins, `queue`
//! comes from the gameflow session, and `win` can come from either path. A
//! panel that only showed values would leave the reader to remember all of
//! that.
//!
//! Read-only. The actions #99 asks for already exist as their own commands —
//! `dev_patch_match_summary`, `dev_trim_lead_in`, `dev_champion_name` — and
//! keeping this a pure read means opening the inspector can never change what
//! it is describing.

use crate::db::RecordingRow;

/// Where one field's value came from, as far as the row can say.
///
/// Derived rather than recorded: nothing stores a per-column writer, so this
/// reasons from what a row looks like. It is therefore a **strong hint, not
/// an audit** — `role` and `patch` only ever come from the deferred patch, so
/// those are certain, while `win` legitimately has two writers that agree.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Provenance {
    pub field: &'static str,
    /// `null` when the field is empty, which is its own answer.
    pub source: Option<&'static str>,
    pub note: &'static str,
}

/// Everything the app knows about one recording.
#[derive(serde::Serialize)]
pub struct RecordingReport {
    pub row: RecordingRow,
    pub provenance: Vec<Provenance>,
    pub markers: i64,
    pub samples: i64,
    /// Samples carrying a gold figure. Zero alongside a healthy `samples`
    /// count is the #137 fingerprint: the live poller ran, the deferred patch
    /// never finished.
    pub gold_samples: i64,
    /// What the markers were mapped through, if anything proved it.
    pub alignment_offset_s: Option<f64>,
    /// `scoreboard_json` parsed, or `null` when there is none or it does not
    /// read — both of which are worth seeing as themselves rather than as an
    /// empty table.
    pub scoreboard: Option<serde_json::Value>,
    pub diagnostics: Option<serde_json::Value>,
}

/// Names the likely writer of each field that has more than one.
///
/// Pure, so the rules are testable without a library. Only the fields whose
/// origin is genuinely ambiguous are listed — a panel that annotated
/// `size_bytes` would be noise.
pub fn provenance_of(row: &RecordingRow) -> Vec<Provenance> {
    let live_or_patch = |present: bool, empty: &'static str| -> (Option<&'static str>, &'static str) {
        if present {
            (Some("live capture, possibly corrected by the LCU patch"), "Written at finalize from Live Client Data, then overwritten by the deferred patch if the LCU disagreed.")
        } else {
            (None, empty)
        }
    };

    let (champion_src, champion_note) = live_or_patch(
        row.champion.is_some(),
        "Empty means the live poller never matched us in `allPlayers` and the LCU could not resolve the id either.",
    );
    let (win_src, win_note) = live_or_patch(
        row.win.is_some(),
        "Empty means neither the live summary nor the LCU said who won.",
    );

    vec![
        Provenance { field: "champion", source: champion_src, note: champion_note },
        Provenance { field: "win", source: win_src, note: win_note },
        Provenance {
            field: "role",
            source: row.role.is_some().then_some("LCU match history"),
            note: "Only the deferred patch ever writes this. Empty means the patch never landed — an app exit inside its retry window, or a game the client has forgotten (#137).",
        },
        Provenance {
            field: "patch",
            source: row.patch.is_some().then_some("LCU match history"),
            note: "Same single writer as `role`; the two are empty together or not at all.",
        },
        Provenance {
            field: "queue",
            source: row.queue.is_some().then_some("gameflow session, confirmed by the LCU"),
            note: "Read when the game started. Empty is normal for Practice Tool and customs, which have no queue.",
        },
        Provenance {
            field: "game_id",
            source: row.game_id.is_some().then_some("gameflow session"),
            note: "Captured during the game. Empty means the gameflow read lost its race, and nothing downstream can ask the LCU about this recording at all.",
        },
        Provenance {
            field: "scoreboard_json",
            source: row.scoreboard_json.is_some().then_some("live capture, replaced by the LCU when it answered"),
            note: "The LCU's version wins where it exists: champion ids rather than display names, and settled numbers (#127).",
        },
        Provenance {
            field: "cs",
            source: row.cs.is_some().then_some("live capture, superseded by the LCU"),
            note: "The live figure is the last poll before the endpoint went away; the LCU's is final.",
        },
        Provenance {
            field: "diagnostics_json",
            source: row.diagnostics_json.is_some().then_some("finalize"),
            note: "What the recording *observed*, as against what it contains. Empty for anything recorded before migration 7 and anything `reconcile` imported.",
        },
    ]
}

/// Assembles the report. Thin: every hard question is answered by a query or
/// by `provenance_of`.
#[tauri::command]
pub fn dev_recording_report(
    state: tauri::State<crate::AppState>,
    recording_id: i64,
) -> Result<RecordingReport, String> {
    let row = state
        .db
        .get_recording(recording_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("no recording {recording_id}"))?;

    let (markers, samples, gold_samples) = state
        .db
        .recording_counts(recording_id)
        .map_err(|e| e.to_string())?;

    // Parsed here rather than in the panel so malformed JSON reads as absent
    // instead of breaking the view — and a column that will not parse is
    // itself a finding worth being able to see the rest around.
    let parse = |json: &Option<String>| -> Option<serde_json::Value> {
        json.as_ref().and_then(|j| serde_json::from_str(j).ok())
    };

    Ok(RecordingReport {
        provenance: provenance_of(&row),
        markers,
        samples,
        gold_samples,
        alignment_offset_s: state
            .db
            .sample_alignment_offset(recording_id)
            .map_err(|e| e.to_string())?,
        scoreboard: parse(&row.scoreboard_json),
        diagnostics: parse(&row.diagnostics_json),
        row,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A row with nothing filled in, which is the interesting starting point:
    /// every field should report *no* source.
    fn row() -> RecordingRow {
        RecordingRow {
            id: 1,
            path: "/a.mp4".into(),
            started_at: 0,
            duration_s: None,
            game_id: None,
            queue: None,
            champion: None,
            role: None,
            win: None,
            kda_k: None,
            kda_d: None,
            kda_a: None,
            patch: None,
            pinned: false,
            size_bytes: 0,
            audio_tracks_json: None,
            game_mode: None,
            diagnostics_json: None,
            scoreboard_json: None,
            cs: None,
        }
    }

    /// An empty field must report *no* source rather than a plausible-looking
    /// one. The whole value of this panel is being able to doubt a value, and
    /// naming a writer for something nothing wrote is the opposite.
    #[test]
    fn an_empty_field_names_no_source() {
        for entry in provenance_of(&row()) {
            assert!(entry.source.is_none(), "{} claimed a source", entry.field);
            assert!(!entry.note.is_empty(), "{} explained nothing", entry.field);
        }
    }

    /// `role` and `patch` have exactly one writer, so their presence is
    /// evidence the deferred patch landed — which is the question actually
    /// being asked when a row looks half-filled.
    #[test]
    fn a_patched_row_names_the_lcu_for_the_fields_only_it_writes() {
        let mut row = row();
        row.role = Some("JUNGLE".into());
        row.patch = Some("16.17".into());

        let found = provenance_of(&row);
        for field in ["role", "patch"] {
            let entry = found.iter().find(|e| e.field == field).unwrap();
            assert_eq!(entry.source, Some("LCU match history"), "{field}");
        }
        // Still nothing wrote the champion.
        let champion = found.iter().find(|e| e.field == "champion").unwrap();
        assert!(champion.source.is_none());
    }

    /// Every ambiguous field is covered, and none twice — a duplicate would
    /// render as two rows disagreeing about the same column.
    #[test]
    fn every_field_appears_exactly_once() {
        let found = provenance_of(&row());
        let mut names: Vec<&str> = found.iter().map(|e| e.field).collect();
        let before = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), before, "a field is listed twice");
        for expected in ["champion", "role", "patch", "queue", "game_id", "win"] {
            assert!(names.contains(&expected), "{expected} is missing");
        }
    }
}
