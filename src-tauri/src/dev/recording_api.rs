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
//! **Read-only, and that is a property worth keeping rather than a stage it
//! passed through.** The inspector does now act on a recording, but through
//! `dev::recording_actions` — a separate module, so the split is visible in
//! the file list and not just in a comment. Everything here reads. Opening the
//! inspector still cannot change what it is describing; only pressing one of
//! the buttons can.

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

/// How a stored value and the client's current answer relate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Both established it and they match.
    Agree,
    /// Both established it and they do not. The finding this whole thing
    /// exists for.
    Differ,
    /// The row has it, the client did not answer. Not a disagreement: match
    /// history ages out, and a custom game was never in it.
    OnlyStored,
    /// The client has it, the row's column is empty — what a deferred patch
    /// that never landed looks like from the outside.
    OnlyLive,
    /// Neither knows. Says the gap is real rather than unasked.
    Neither,
}

/// One field, as the row holds it and as the client reports it now.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct FieldComparison {
    pub field: &'static str,
    /// Rendered as text so one shape covers every column, and so the panel
    /// never has to know a column's type to show it.
    pub stored: Option<String>,
    pub live: Option<String>,
    pub verdict: Verdict,
}

fn verdict(stored: &Option<String>, live: &Option<String>) -> Verdict {
    match (stored, live) {
        (Some(a), Some(b)) if a == b => Verdict::Agree,
        (Some(_), Some(_)) => Verdict::Differ,
        (Some(_), None) => Verdict::OnlyStored,
        (None, Some(_)) => Verdict::OnlyLive,
        (None, None) => Verdict::Neither,
    }
}

/// The row beside what the client says about the same game, field by field.
///
/// **Pure, and the only place the comparison lives.** The command below is a
/// fetch and a call to this, so the rule about what counts as a disagreement
/// is unit-tested rather than exercised by playing a game.
///
/// `champion` is the LCU's champion *id* already resolved to a name, because
/// that is the only form comparable to what the column holds — the row stores
/// `Wukong`, never `MonkeyKing` or `62` (DEVELOPMENT.md §3.1). A resolution
/// that failed arrives as `None` and reads as "the client did not say",
/// which is honest: an id we cannot name is not evidence of disagreement.
///
/// `kda` is compared as one field rather than three. It is written as a unit
/// by both writers and `formatKda` already refuses to show a partial one, so
/// three rows saying `Differ` about one event would overstate the finding.
pub fn compare_with_summary(
    row: &crate::db::RecordingRow,
    summary: &crate::lcu::MatchSummary,
    champion: Option<String>,
) -> Vec<FieldComparison> {
    let kda = |k: Option<i64>, d: Option<i64>, a: Option<i64>| match (k, d, a) {
        (Some(k), Some(d), Some(a)) => Some(format!("{k}/{d}/{a}")),
        _ => None,
    };
    let text = |v: &Option<String>| v.clone();
    let num = |v: Option<i64>| v.map(|n| n.to_string());
    let outcome = |v: Option<bool>| v.map(|w| if w { "Win" } else { "Loss" }.to_string());

    let pairs: Vec<(&'static str, Option<String>, Option<String>)> = vec![
        ("game_id", num(row.game_id), num(summary.game_id)),
        ("champion", text(&row.champion), champion),
        ("queue", num(row.queue), num(summary.queue_id)),
        ("role", text(&row.role), summary.role.clone()),
        ("patch", text(&row.patch), summary.patch.clone()),
        ("win", outcome(row.win), outcome(summary.win)),
        (
            "kda",
            kda(row.kda_k, row.kda_d, row.kda_a),
            kda(summary.kills, summary.deaths, summary.assists),
        ),
    ];

    pairs
        .into_iter()
        .map(|(field, stored, live)| FieldComparison {
            verdict: verdict(&stored, &live),
            field,
            stored,
            live,
        })
        .collect()
}

/// What the client says about a recording's game, right now.
#[derive(serde::Serialize)]
pub struct LcuComparison {
    /// The game asked about, so the answer can never be read as being about
    /// a different row than the one in front of you.
    pub game_id: i64,
    pub fields: Vec<FieldComparison>,
    /// How many fields came back `Differ`, so the panel can lead with the
    /// answer rather than making someone scan for it.
    pub differing: usize,
}

/// Asks the client about a recording's game and lays its answer beside the
/// row's, field by field.
///
/// **The same `fetch_match_summary` the deferred patch uses**, with no retry
/// schedule — one shot, because this is a question somebody asked rather than
/// a patch that has to land. A disagreement here almost always means the
/// wrong `game_id` was matched, which is exactly the thing that silently
/// mislabels a library (#99).
///
/// Read-only, like the report beside it: it writes nothing back, so asking
/// can never change what it describes. `dev_patch_match_summary` is the one
/// that acts on the answer.
#[tauri::command]
pub async fn dev_recording_vs_lcu(
    state: tauri::State<'_, crate::AppState>,
    recording_id: i64,
) -> Result<LcuComparison, String> {
    let row = state
        .db
        .get_recording(recording_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("no recording {recording_id}"))?;

    // Without one there is nothing to ask about, and that is a finding in
    // itself rather than an error to paper over: a row with no `game_id` was
    // never matched to a game, so no amount of asking will describe it.
    let game_id = row
        .game_id
        .ok_or_else(|| "this recording has no game id, so there is nothing to ask the client about".to_string())?;

    let lockfile = crate::lcu::lockfile::discover()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "League Client not running (no lockfile found)".to_string())?;
    let client = crate::lcu::LcuHttpClient::new(&lockfile).map_err(|e| e.to_string())?;

    let summary = crate::lcu::fetch_match_summary(&client, game_id, false)
        .await
        .map_err(|e| e.to_string())?;

    // Resolved through the same path the patch uses, so the name compared is
    // the one that would actually have been written.
    let champion = match summary.champion_id {
        Some(id) => crate::lcu::champion_name(&client, &lockfile, id).await,
        None => None,
    };

    let fields = compare_with_summary(&row, &summary, champion);
    let differing = fields.iter().filter(|f| f.verdict == Verdict::Differ).count();
    Ok(LcuComparison { game_id, fields, differing })
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

    // --- the LCU comparison ------------------------------------------

    fn summary() -> crate::lcu::MatchSummary {
        crate::lcu::MatchSummary::default()
    }

    fn find<'a>(fields: &'a [FieldComparison], name: &str) -> &'a FieldComparison {
        fields.iter().find(|f| f.field == name).expect("field is compared")
    }

    /// The finding the whole thing exists for: two sources that both answered
    /// and answered differently, which almost always means the wrong game id
    /// was matched.
    #[test]
    fn a_field_both_sources_answered_differently_is_a_disagreement() {
        let mut row = row();
        row.role = Some("Jungle".into());
        let mut summary = summary();
        summary.role = Some("Top".into());

        let fields = compare_with_summary(&row, &summary, None);
        let role = find(&fields, "role");
        assert_eq!(role.verdict, Verdict::Differ);
        assert_eq!(role.stored.as_deref(), Some("Jungle"));
        assert_eq!(role.live.as_deref(), Some("Top"));
    }

    /// A gap on one side is not a disagreement, and the two directions mean
    /// opposite things — one is a patch that never landed, the other is match
    /// history having aged the game out.
    #[test]
    fn a_gap_on_one_side_is_not_a_disagreement() {
        let mut stored_only = row();
        stored_only.patch = Some("15.3.412".into());
        let fields = compare_with_summary(&stored_only, &summary(), None);
        assert_eq!(find(&fields, "patch").verdict, Verdict::OnlyStored);

        let mut live_only = summary();
        live_only.patch = Some("15.3.412".into());
        let fields = compare_with_summary(&row(), &live_only, None);
        assert_eq!(find(&fields, "patch").verdict, Verdict::OnlyLive);
    }

    /// Neither knowing is its own answer, not a silent pass.
    #[test]
    fn a_field_neither_source_knows_says_so() {
        let fields = compare_with_summary(&row(), &summary(), None);
        assert_eq!(find(&fields, "queue").verdict, Verdict::Neither);
    }

    /// KDA is one field, not three. Both writers write it as a unit, so
    /// three rows saying `Differ` about one event would overstate it.
    #[test]
    fn kda_is_compared_as_one_field_and_needs_all_three() {
        let mut row = row();
        row.kda_k = Some(7);
        row.kda_d = Some(2);
        row.kda_a = Some(5);
        let mut summary = summary();
        summary.kills = Some(7);
        summary.deaths = Some(3);
        summary.assists = Some(5);

        let fields = compare_with_summary(&row, &summary, None);
        assert_eq!(fields.iter().filter(|f| f.field == "kda").count(), 1);
        let kda = find(&fields, "kda");
        assert_eq!(kda.verdict, Verdict::Differ);
        assert_eq!(kda.stored.as_deref(), Some("7/2/5"));
        assert_eq!(kda.live.as_deref(), Some("7/3/5"));

        // A partial KDA is not half an answer — same rule `formatKda` applies.
        let mut partial = row.clone();
        partial.kda_d = None;
        let fields = compare_with_summary(&partial, &summary, None);
        assert_eq!(find(&fields, "kda").verdict, Verdict::OnlyLive);
    }

    /// The champion is compared as a *name*, because that is the only form
    /// the column holds. An id the client could not name arrives as `None`
    /// and reads as "the client did not say" — an unnameable id is not
    /// evidence that the row is wrong.
    #[test]
    fn an_unresolvable_champion_id_is_not_a_disagreement() {
        let mut row = row();
        row.champion = Some("Wukong".into());
        let mut summary = summary();
        summary.champion_id = Some(62);

        let fields = compare_with_summary(&row, &summary, None);
        assert_eq!(find(&fields, "champion").verdict, Verdict::OnlyStored);

        let fields = compare_with_summary(&row, &summary, Some("Wukong".into()));
        assert_eq!(find(&fields, "champion").verdict, Verdict::Agree);
    }

    /// Agreement is the common case and has to be reported as such, or the
    /// panel could not lead with a count of what differs.
    #[test]
    fn matching_values_agree() {
        let mut row = row();
        row.win = Some(true);
        row.queue = Some(420);
        let mut summary = summary();
        summary.win = Some(true);
        summary.queue_id = Some(420);

        let fields = compare_with_summary(&row, &summary, None);
        assert_eq!(find(&fields, "win").verdict, Verdict::Agree);
        assert_eq!(find(&fields, "queue").verdict, Verdict::Agree);
        assert_eq!(find(&fields, "win").stored.as_deref(), Some("Win"));
    }
}

/// A player's ranked standing, as the client reports it right now.
///
/// The probe half of #149: `puuid` omitted asks about us
/// (`current-ranked-stats`), and given one asks about anybody else
/// (`ranked-stats/{puuid}`). Both return the same document, so one command
/// covers the pair.
///
/// **Take the puuid from a match document, not from an alias lookup.**
/// `/lol-summoner/v1/alias/lookup` answers with a *name-derived* UUID — a v5,
/// hashed from the alias rather than assigned to an account — and the ranked
/// ladder has nothing keyed by it. The lookup succeeds and this returns
/// nothing, which looks identical to an unranked player and means something
/// completely different.
///
/// Returns the raw document beside the parsed standings, because the whole
/// point of a probe is seeing what actually arrived.
#[tauri::command]
pub async fn dev_ranked_stats(puuid: Option<String>) -> Result<serde_json::Value, String> {
    let lockfile = crate::lcu::lockfile::discover()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "League Client not running (no lockfile found)".to_string())?;
    let client = crate::lcu::LcuHttpClient::new(&lockfile).map_err(|e| e.to_string())?;

    let path = match puuid.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
        Some(puuid) => format!("/lol-ranked/v1/ranked-stats/{puuid}"),
        None => "/lol-ranked/v1/current-ranked-stats".to_string(),
    };

    let raw: serde_json::Value = client.get_json(&path).await.map_err(|e| e.to_string())?;
    let stats: crate::lcu::ranked::RankedStats =
        serde_json::from_value(raw.clone()).map_err(|e| e.to_string())?;

    Ok(serde_json::json!({
        "asked": path,
        "solo": stats.standing(crate::lcu::ranked::SOLO),
        "flex": stats.standing(crate::lcu::ranked::FLEX),
        "raw": raw,
    }))
}

/// A whole lobby's rank, from the puuids of the players in it.
///
/// The probe for #149's Decision 3, against a real game rather than a
/// constructed one. Take the puuids from a captured `eog-stats-block`
/// (`teams[].players[].puuid`) — it carries all ten, and the deferred patch
/// already fetches it, which is why the shipped feature will need no extra
/// request to identify a lobby.
///
/// One request per player, run in sequence. Ten calls to a process on
/// localhost is not worth a concurrency primitive, and a client mid-shutdown
/// is happier with a queue than a burst.
///
/// **A player whose rank cannot be read is excluded, not counted low**, and
/// the result says how many it knew. That is the whole difference between
/// "Gold II" and "Gold II, 8 of 10" — one is a claim about a lobby, the other
/// is a claim about eight people in it.
#[tauri::command]
pub async fn dev_lobby_rank(
    puuids: Vec<String>,
    queue: Option<String>,
) -> Result<serde_json::Value, String> {
    use crate::lcu::ranked;

    let lockfile = crate::lcu::lockfile::discover()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "League Client not running (no lockfile found)".to_string())?;
    let client = crate::lcu::LcuHttpClient::new(&lockfile).map_err(|e| e.to_string())?;
    let queue = queue.unwrap_or_else(|| ranked::SOLO.to_string());

    let mut standings = Vec::with_capacity(puuids.len());
    let mut failed = Vec::new();
    for puuid in &puuids {
        let path = format!("/lol-ranked/v1/ranked-stats/{}", puuid.trim());
        match client.get_json::<ranked::RankedStats>(&path).await {
            Ok(stats) => standings.push(stats.standing(&queue)),
            Err(e) => {
                // A lookup that fails is a player we do not know about, which
                // is the same as an unranked one for the median's purposes —
                // but it is worth reporting separately, because a run where
                // every lookup failed is a broken probe, not an unranked lobby.
                failed.push(format!("{}: {e}", puuid.trim()));
                standings.push(None);
            }
        }
    }

    Ok(serde_json::json!({
        "queue": queue,
        "asked": puuids.len(),
        "failed": failed,
        "standings": standings,
        "lobby_rank": ranked::lobby_rank(&standings),
        "min_known": ranked::MIN_KNOWN,
    }))
}
