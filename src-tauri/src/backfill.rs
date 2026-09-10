//! Labelling the recordings that predate the metadata pipeline.
//! DEVELOPMENT.md §3.1, §4.
//!
//! Everything in #49 only fixes recordings made *after* it shipped. The
//! games already in the library — and anything `reconcile` imported from a
//! folder the user pointed at — keep NULL `champion`, `win`, `queue` and
//! the rest forever, which is most of a real library and therefore most of
//! what the win-rate tile is computed from.
//!
//! There is no game id to ask about here. A finalize captures one *during*
//! the game (`lcu::fetch_session`), which is exactly the thing these rows
//! never had, so the only handle left is the clock: a recording ran from
//! some instant for some length, and so did a game.
//!
//! ## Matching on time, and refusing when it is close
//!
//! `match_recording` is pure and directly tested. It accepts a game only
//! when the two windows genuinely overlap, and **refuses outright when more
//! than one game qualifies** rather than picking the best. A card labelled
//! with the wrong game is worse than one labelled `—`: the whole value of
//! this library is that what it says about a VOD is true, and a wrong
//! label is invisible — nobody re-checks a row that looks plausible.
//!
//! ## Manual, and one pass
//!
//! Triggered by the user, never on startup. It is a bulk operation against
//! their running client, and the moment to do it is theirs to pick. One
//! request for the history and one for the summoner cover the whole run:
//! the list response carries entire game documents, so forty unlabelled
//! recordings cost two requests rather than forty-two.
//!
//! It reuses the pieces the deferred patch already built — `to_metadata`,
//! `champion_name`, `update_match_metadata` — and adds no second answer to
//! any question they already answer.

use crate::db::{Db, MatchMetadata};
use crate::lcu::{self, LcuHttpClient, LockfileInfo, PlayedGame};
use crate::live_client::Scoreboard;
use crate::match_summary::to_metadata;
use crate::{info, warn};
use serde::Serialize;

/// How many games back to ask the client for. Two hundred is several
/// months of ordinary play and one request; a library holding recordings
/// older than the client's own history simply will not match them, which
/// is the honest outcome rather than something to page around for.
const HISTORY_DEPTH: u32 = 200;

/// The fraction of the shorter window that has to overlap before a game is
/// accepted as the one a recording holds.
///
/// Not "any overlap": recording starts a little before the game and stops a
/// little after, so consecutive games in one session can brush each other
/// at the edges. Requiring half rules that out while staying far away from
/// demanding the two agree exactly — they never will, because a recording
/// includes the loading screen and the client's `gameDuration` does not.
const MIN_OVERLAP: f64 = 0.5;

/// A recording with no match metadata, reduced to what matching needs.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub id: i64,
    /// Epoch milliseconds.
    pub started_at: i64,
    pub duration_s: Option<f64>,
    /// Matched to a game, but carrying no gold curve.
    ///
    /// Its own flag rather than something inferred from the columns above: a
    /// recording can be complete in every other respect and still have lost
    /// its curve, because the gold series comes from a deferred patch that
    /// only ever lived in memory (#137).
    pub needs_gold: bool,
}

/// What the clock can say about one recording.
#[derive(Debug, Clone, PartialEq)]
pub enum Match {
    /// Exactly one game overlaps. The only case that writes anything.
    One(i64),
    /// Nothing overlaps: a game older than the history window, a custom
    /// that never reached match history, or a video that was never a game.
    NoneFound,
    /// More than one game overlaps, so the clock cannot say which. Refused
    /// on purpose — see the module header.
    Ambiguous,
}

/// Milliseconds of overlap between two half-open windows.
fn overlap_ms(a_start: i64, a_len: i64, b_start: i64, b_len: i64) -> i64 {
    let end = (a_start + a_len).min(b_start + b_len);
    let start = a_start.max(b_start);
    (end - start).max(0)
}

/// Whether `game` is plausibly the game `recording` holds.
///
/// A recording with no known duration is treated as an instant rather than
/// a window — `reconcile`-imported files can arrive without one, and the
/// honest thing to ask of a bare timestamp is whether it falls inside a
/// game, not to invent a length for it.
fn windows_overlap(recording: &Candidate, game: &PlayedGame) -> bool {
    let Some(game_start) = game.started_at else {
        return false;
    };
    let game_len = game.duration_s.unwrap_or(0).saturating_mul(1000);
    if game_len <= 0 {
        return false;
    }

    let recording_len = recording
        .duration_s
        .filter(|s| *s > 0.0)
        .map(|s| (s * 1000.0) as i64);

    let Some(recording_len) = recording_len else {
        return recording.started_at >= game_start && recording.started_at < game_start + game_len;
    };

    let overlap = overlap_ms(recording.started_at, recording_len, game_start, game_len);
    let shorter = recording_len.min(game_len);
    overlap as f64 / shorter as f64 >= MIN_OVERLAP
}

/// Which game a recording holds, or why the clock cannot say.
pub fn match_recording(recording: &Candidate, games: &[PlayedGame]) -> Match {
    let mut hit: Option<i64> = None;
    for game in games.iter().filter(|g| windows_overlap(recording, g)) {
        if hit.is_some() {
            return Match::Ambiguous;
        }
        hit = Some(game.game_id);
    }
    match hit {
        Some(game_id) => Match::One(game_id),
        None => Match::NoneFound,
    }
}

/// What one pass did. Every count is reported rather than only the
/// successes: "nothing happened" and "nothing could be matched" are
/// different answers, and the second one is the user's cue that their
/// recordings are older than their client's match history.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct BackfillReport {
    /// Rows that were missing metadata when the pass started.
    pub scanned: usize,
    /// Games the client offered.
    pub games_considered: usize,
    /// Rows the clock matched to exactly one game.
    pub matched: usize,
    /// Rows actually written. Lower than `matched` when a row was deleted
    /// mid-pass, or when the matched game said nothing worth writing.
    pub patched: usize,
    /// Rows where more than one game overlapped, so nothing was written.
    pub ambiguous: usize,
    /// Rows no game overlapped.
    pub unmatched: usize,
    /// Rows that regained a gold curve. Separate from `patched` because the
    /// two fail independently: the timeline is a different endpoint, and a
    /// game old enough to have fallen out of match history can still yield
    /// metadata while having no timeline left to fetch.
    pub gold_filled: usize,
    /// Rows that gained a scoreboard they did not have — every recording
    /// made before the live capture existed.
    pub scoreboards: usize,
}

/// Runs one backfill pass against the running client.
///
/// Thin on purpose: the decision is `match_recording`, the mapping is
/// `to_metadata`, the write is `update_match_metadata`, and all three are
/// tested where they live.
pub async fn run(db: &Db) -> Result<BackfillReport, String> {
    let candidates = db
        .recordings_missing_metadata()
        .map_err(|e| format!("could not read the library: {e}"))?;
    run_for(db, candidates).await
}

/// The same pass, against one recording.
///
/// For the inspector (#99), where the question is about the row in front of
/// you rather than the library. It goes through the *same* candidate query
/// and the same loop rather than a path of its own, so a row the bulk run
/// would skip is skipped here too and for the same reason — a second
/// implementation would be a second set of rules to disagree with the first.
///
/// A recording that is not a candidate comes back with `scanned: 0`, which is
/// the honest answer: there is nothing here the backfill can fill.
///
/// Only the dev portal's inspector calls this, and clippy runs without
/// `--all-targets`, so in a shipped build it is genuinely dead code
/// (CLAUDE.md).
#[cfg_attr(not(feature = "devtools"), allow(dead_code))]
pub async fn run_one(db: &Db, recording_id: i64) -> Result<BackfillReport, String> {
    let candidates: Vec<Candidate> = db
        .recordings_missing_metadata()
        .map_err(|e| format!("could not read the library: {e}"))?
        .into_iter()
        .filter(|c| c.id == recording_id)
        .collect();
    run_for(db, candidates).await
}

/// The pass itself. One request for the history covers every candidate given.
async fn run_for(db: &Db, candidates: Vec<Candidate>) -> Result<BackfillReport, String> {
    let mut report = BackfillReport {
        scanned: candidates.len(),
        ..Default::default()
    };
    if candidates.is_empty() {
        return Ok(report);
    }

    let lockfile = lcu::lockfile::discover()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "League Client not running — the backfill reads its match history".to_string())?;
    let client = lcu::LcuHttpClient::new(&lockfile).map_err(|e| e.to_string())?;

    let games = lcu::fetch_recent_games(&client, HISTORY_DEPTH)
        .await
        .map_err(|e| format!("could not read match history: {e}"))?;
    report.games_considered = games.len();

    for candidate in &candidates {
        let game_id = match match_recording(candidate, &games) {
            Match::One(game_id) => game_id,
            Match::Ambiguous => {
                report.ambiguous += 1;
                warn!("backfill", "recording {} overlaps more than one game; leaving it alone",
                    candidate.id
                );
                continue;
            }
            Match::NoneFound => {
                report.unmatched += 1;
                continue;
            }
        };
        report.matched += 1;

        let Some(game) = games.iter().find(|g| g.game_id == game_id) else {
            continue;
        };

        // Best effort, exactly as in the deferred patch: a name that will
        // not resolve must not cost the row its outcome and queue id.
        let champion = match game.summary.champion_id {
            Some(id) => lcu::champion_name(&client, &lockfile, id).await,
            None => None,
        };

        // Before the metadata write, matching the order the deferred patch
        // uses: it is the slower half, and its failures are logged and
        // swallowed rather than costing the row its outcome.
        if candidate.needs_gold
            && crate::match_summary::write_gold_series(db, &client, candidate.id, game_id).await
        {
            report.gold_filled += 1;
        }

        let metadata: MatchMetadata = to_metadata(&game.summary, champion);
        match db.update_match_metadata(candidate.id, &metadata) {
            // Zero rows is the row having been deleted since the scan —
            // normal, and not something to count as a failure.
            Ok(0) => {}
            Ok(_) => report.patched += 1,
            Err(e) => warn!("backfill", "could not patch recording {}: {e}", candidate.id),
        }

        if fill_scoreboard(db, &client, &lockfile, candidate.id, game).await {
            report.scoreboards += 1;
        }
    }

    info!("backfill",
        "{} of {} unlabelled recordings matched against {} games; {} written, \
         {} scoreboards rebuilt, {} ambiguous",
        report.matched,
        report.scanned,
        report.games_considered,
        report.patched,
        report.scoreboards,
        report.ambiguous
    );
    Ok(report)
}

/// Rebuilds a scoreboard for one recording out of the match-history
/// document, if it has none.
///
/// Returns whether one was written. Every early return leaves the row as
/// it was — a recording without a scoreboard still has its champion, KDA
/// and result, and the row renders without one.
async fn fill_scoreboard(
    db: &Db,
    client: &LcuHttpClient,
    lockfile: &LockfileInfo,
    recording_id: i64,
    game: &PlayedGame,
) -> bool {
    if game.participants.is_empty() {
        return false;
    }

    let mut players = Vec::with_capacity(game.participants.len());
    for participant in &game.participants {
        players.push(crate::match_summary::scoreboard_player(client, lockfile, participant).await);
    }

    let us = game.participants.iter().find(|p| p.is_us);
    let scoreboard = Scoreboard {
        our_team: us.and_then(|p| p.team.clone()),
        our_runes: us.and_then(crate::match_summary::runes_of),
        players,
    };

    let json = match serde_json::to_string(&scoreboard) {
        Ok(json) => json,
        Err(e) => {
            warn!("backfill", "could not serialize a scoreboard for {recording_id}: {e}");
            return false;
        }
    };

    match db.fill_scoreboard(recording_id, &json, us.and_then(|p| p.cs)) {
        Ok(written) => written,
        Err(e) => {
            warn!("backfill", "could not write a scoreboard for {recording_id}: {e}");
            false
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::lcu::MatchSummary;

    const MINUTE: i64 = 60_000;

    fn game(game_id: i64, started_at: i64, duration_s: i64) -> PlayedGame {
        PlayedGame {
            game_id,
            started_at: Some(started_at),
            duration_s: Some(duration_s),
            summary: MatchSummary {
                game_id: Some(game_id),
                ..Default::default()
            },
            participants: Vec::new(),
        }
    }

    fn recording(started_at: i64, duration_s: Option<f64>) -> Candidate {
        Candidate {
            needs_gold: false,
            id: 1,
            started_at,
            duration_s,
        }
    }

    /// The ordinary case: a recording starts a few seconds before the game
    /// clock does and stops a few after, because it covers the loading
    /// screen and the client's `gameDuration` does not.
    #[test]
    fn a_recording_matches_the_game_it_brackets() {
        let games = [game(555, 10 * MINUTE, 1800)];
        let rec = recording(10 * MINUTE - 20_000, Some(1840.0));
        assert_eq!(match_recording(&rec, &games), Match::One(555));
    }

    /// Two games back to back in one session. Neither is a candidate for
    /// the other's recording, and the pass must label both.
    #[test]
    fn consecutive_games_do_not_shadow_each_other() {
        let games = [
            game(1, 0, 1800),
            game(2, 40 * MINUTE, 1800),
        ];
        assert_eq!(match_recording(&recording(0, Some(1800.0)), &games), Match::One(1));
        assert_eq!(
            match_recording(&recording(40 * MINUTE, Some(1800.0)), &games),
            Match::One(2)
        );
    }

    /// The case the whole module exists to get right. A recording that
    /// straddles two games is not a hard call to make well — it is a call
    /// that must not be made at all.
    #[test]
    fn a_recording_spanning_two_games_is_refused_not_guessed() {
        let games = [game(1, 0, 3600), game(2, 30 * MINUTE, 3600)];
        let rec = recording(0, Some(5400.0));
        assert_eq!(match_recording(&rec, &games), Match::Ambiguous);
    }

    /// Brushing at the edges is not a match. A recording that ran three
    /// seconds into the next game has not recorded any of it.
    #[test]
    fn a_few_seconds_of_overlap_is_not_a_match() {
        let games = [game(2, 30 * MINUTE, 1800)];
        let rec = recording(28 * MINUTE, Some(123.0));
        assert_eq!(match_recording(&rec, &games), Match::NoneFound);
    }

    #[test]
    fn a_recording_older_than_the_history_matches_nothing() {
        let games = [game(1, 100 * MINUTE, 1800)];
        assert_eq!(
            match_recording(&recording(0, Some(1800.0)), &games),
            Match::NoneFound
        );
    }

    /// `reconcile` imports files without one. A bare timestamp is asked the
    /// only question it can answer: does it fall inside a game?
    #[test]
    fn a_recording_with_no_duration_matches_on_its_start_alone() {
        let games = [game(7, 10 * MINUTE, 1800)];
        assert_eq!(
            match_recording(&recording(20 * MINUTE, None), &games),
            Match::One(7)
        );
        assert_eq!(
            match_recording(&recording(9 * MINUTE, None), &games),
            Match::NoneFound
        );
    }

    /// A client that answered without a creation time or a length cannot
    /// be matched against, and must not become a wildcard that swallows
    /// every recording.
    #[test]
    fn a_game_with_no_clock_is_never_a_match() {
        let games = [PlayedGame {
            game_id: 9,
            started_at: None,
            duration_s: None,
            summary: MatchSummary::default(),
            participants: Vec::new(),
        }];
        assert_eq!(
            match_recording(&recording(10 * MINUTE, Some(1800.0)), &games),
            Match::NoneFound
        );
    }
}
