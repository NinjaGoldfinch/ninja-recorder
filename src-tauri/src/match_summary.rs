//! Fills in a recording's post-game metadata after it has already been
//! finalized. DEVELOPMENT.md §3.1, §4.
//!
//! ## Why this is not part of the finalize
//!
//! At the instant `Recording → Finalizing` fires, the LCU is still in
//! `WaitingForStats`: `/lol-match-history/v1/games/{gameId}` 404s or
//! answers with a stats block that has not been filled in yet. The same
//! transition also emits `StopGameflowWatch`, so the task that owned the
//! LCU connection is being torn down in the same breath. Fetching inline
//! would block the finalize behind a request that is *expected* to fail.
//!
//! So the row is written exactly as before, from what Live Client Data
//! established during the game, and this patches it afterwards — then the
//! frontend is told the library changed, and the card fills itself in
//! while the user is looking at it.
//!
//! ## Shape
//!
//! Pure decision, thin I/O wrapper (CLAUDE.md). The schedule
//! (`next_delay`), the retryable/hopeless split (`status_is_transient`),
//! the summary→columns mapping (`to_metadata`) and the live-vs-LCU
//! comparison (`disagreements`) are all pure and directly unit-tested;
//! `patch` is the loop that calls them and is deliberately too small to
//! hide a decision.
//!
//! Failure here is silent by design past the last attempt. The row already
//! carries champion, KDA and outcome from the live path — a missing queue
//! id is not worth interrupting somebody's next game over.

use crate::db::{Db, MatchMetadata};
use crate::lcu::{self, MatchDataError, MatchSummary};
use crate::live_client::LiveSummary;
use std::time::Duration;

/// Everything the patch needs, handed over at finalize.
///
/// A plain data struct on purpose: the supervisor builds one and passes it
/// to a type-erased callback, so it never has to know that an HTTP client,
/// an async runtime or a retry loop exist. See
/// `state_machine::Supervisor::summary_fetcher`.
#[derive(Debug, Clone)]
pub struct SummaryRequest {
    pub recording_id: i64,
    pub game_id: i64,
    /// A custom game never reaches match history, so asking for it costs a
    /// request and a full retry cycle for a 404 that will never become a
    /// 200. The end-of-game block still answers for one.
    pub is_custom: bool,
    /// Which client to ask. Stashed by the supervisor when the gameflow
    /// watch started rather than rediscovered here: the client can restart
    /// between the game and this patch, and a fresh `discover` would then
    /// answer about a different process than the one that played the game.
    pub lockfile: lcu::LockfileInfo,
    /// What Live Client Data recorded, so a disagreement can be reported.
    /// Not used to *write* anything — the row already has these.
    pub live: LiveSummary,
}

/// How long to wait before attempt `attempt + 1`, or `None` to give up.
///
/// Roughly 2s, 4s, 8s, then three at 15s — about a 60s ceiling. The front
/// of the schedule is tight because `eog-stats-block` is populated during
/// the `EndOfGame` phase and should answer almost immediately; the tail is
/// flat because if it hasn't by then, the client is doing something slow
/// and hammering it will not help.
pub fn next_delay(attempt: u32) -> Option<Duration> {
    const SCHEDULE_S: [u64; 6] = [2, 4, 8, 15, 15, 15];
    SCHEDULE_S
        .get(attempt as usize)
        .copied()
        .map(Duration::from_secs)
}

/// Whether an HTTP failure with this status is worth waiting out.
///
/// `None` means the request never got a response at all. That is the
/// client being gone, not the stats being late — retrying a dead client
/// for a minute is pointless.
pub fn status_is_transient(status: Option<u16>) -> bool {
    match status {
        // The game is over but the client hasn't written the stats yet.
        // This is the normal answer for the first few seconds and is
        // exactly what the schedule above exists for.
        Some(404) | Some(204) => true,
        Some(status) if status >= 500 => true,
        // 401/403 and friends: our credentials are wrong or the endpoint
        // moved. Neither improves by asking again.
        Some(_) => false,
        None => false,
    }
}

/// The same question for a whole fetch, not just an HTTP status.
fn worth_retrying(error: &MatchDataError) -> bool {
    match error {
        // "The stats aren't there yet" is the literal meaning of this one.
        MatchDataError::NotReady(_) => true,
        // The game exists but we are not in its participant list. Treated
        // as transient rather than fatal because that is what a
        // half-written match-history document looks like — the identities
        // arrive anonymised or empty before the client fills them in. The
        // schedule terminates either way, so being wrong costs a minute of
        // idle polling and never a wrong answer.
        MatchDataError::ParticipantNotFound(_) => true,
        MatchDataError::Client(lcu::LcuClientError::Request(e)) => {
            status_is_transient(e.status().map(|s| s.as_u16()))
        }
        // A body we cannot parse is the wrong *shape*, and shapes don't
        // change while the client runs.
        MatchDataError::Client(_) => false,
    }
}

/// The columns this patch is allowed to write.
///
/// `champion` is deliberately absent: the LCU answers with a champion
/// *id*, and turning one into the same display name Live Client Data
/// writes needs the lookup that is still to come. Writing the internal
/// alias in the meantime would split one champion into two everywhere the
/// library sorts and filters — see `db::MatchMetadata::champion`.
pub fn to_metadata(summary: &MatchSummary) -> MatchMetadata {
    MatchMetadata {
        game_id: summary.game_id,
        queue: summary.queue_id,
        role: summary.role.clone(),
        patch: summary.patch.clone(),
        win: summary.win,
        kda_k: summary.kills,
        kda_d: summary.deaths,
        kda_a: summary.assists,
        champion: None,
    }
}

/// Where the LCU and Live Client Data describe the same game differently.
///
/// They should never differ — they are two views of one match — so a
/// disagreement almost certainly means the wrong `gameId` was matched, and
/// that is worth knowing *before* it silently mislabels a library. Only
/// compares fields both sources actually established.
pub fn disagreements(live: &LiveSummary, summary: &MatchSummary) -> Vec<String> {
    let mut out = Vec::new();
    if let (Some(live_win), Some(lcu_win)) = (live.win, summary.win) {
        if live_win != lcu_win {
            out.push(format!(
                "outcome: the game reported {}, the client reports {}",
                outcome(live_win),
                outcome(lcu_win)
            ));
        }
    }
    if let (Some(live_kda), (Some(k), Some(d), Some(a))) =
        (live.kda, (summary.kills, summary.deaths, summary.assists))
    {
        if (live_kda.kills, live_kda.deaths, live_kda.assists) != (k, d, a) {
            out.push(format!(
                "KDA: the game reported {}/{}/{}, the client reports {}/{}/{}",
                live_kda.kills, live_kda.deaths, live_kda.assists, k, d, a
            ));
        }
    }
    out
}

fn outcome(win: bool) -> &'static str {
    if win {
        "a win"
    } else {
        "a loss"
    }
}

/// Fetches the summary for `request.game_id`, retrying on the schedule
/// above, and patches the recording's row with it.
///
/// Returns whether a patch actually landed, so the caller knows whether
/// telling the frontend the library changed would say anything.
pub async fn patch(db: &Db, request: &SummaryRequest) -> bool {
    let client = match lcu::LcuHttpClient::new(&request.lockfile) {
        Ok(client) => client,
        Err(e) => {
            eprintln!("[match-summary] could not build an LCU client: {e}");
            return false;
        }
    };

    let mut attempt = 0;
    let summary = loop {
        match lcu::fetch_match_summary(&client, request.game_id, request.is_custom).await {
            Ok(summary) => break summary,
            Err(e) => {
                if !worth_retrying(&e) {
                    eprintln!(
                        "[match-summary] giving up on game {}: {e}",
                        request.game_id
                    );
                    return false;
                }
                match next_delay(attempt) {
                    Some(delay) => {
                        tokio::time::sleep(delay).await;
                        attempt += 1;
                    }
                    // Out of attempts. Silent on purpose: the row already
                    // carries what the live client saw, and the backfill
                    // pass exists to sweep these up later.
                    None => return false,
                }
            }
        }
    };

    for note in disagreements(&request.live, &summary) {
        eprintln!(
            "[match-summary] game {} disagrees with what was recorded live — {note}. \
             The most likely cause is the wrong game being matched.",
            request.game_id
        );
    }

    match db.update_match_metadata(request.recording_id, &to_metadata(&summary)) {
        // The row was deleted between the finalize and now — retention
        // runs during the same finalize, and the user can delete a card at
        // any point. Nothing went wrong; there is just nothing to say.
        Ok(0) => false,
        Ok(_) => {
            println!(
                "[match-summary] patched recording {} from game {}",
                request.recording_id, request.game_id
            );
            true
        }
        Err(e) => {
            eprintln!(
                "[match-summary] could not patch recording {}: {e}",
                request.recording_id
            );
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::NewRecording;
    use crate::live_client::Kda;

    // --- the schedule -----------------------------------------------------

    #[test]
    fn the_schedule_starts_tight_and_then_flattens() {
        assert_eq!(next_delay(0), Some(Duration::from_secs(2)));
        assert_eq!(next_delay(1), Some(Duration::from_secs(4)));
        assert_eq!(next_delay(2), Some(Duration::from_secs(8)));
        assert_eq!(next_delay(3), Some(Duration::from_secs(15)));
    }

    /// The point of a schedule rather than a loop: it has to stop. A
    /// retry that never terminates is a task per game, forever, on a
    /// process the user leaves running all evening.
    #[test]
    fn the_schedule_terminates() {
        assert_eq!(next_delay(6), None);
        assert_eq!(next_delay(1000), None);
    }

    #[test]
    fn the_whole_schedule_is_about_a_minute() {
        let total: u64 = (0..)
            .map_while(next_delay)
            .map(|d| d.as_secs())
            .sum();
        assert_eq!(total, 59);
    }

    // --- transient vs hopeless -------------------------------------------

    /// The normal answer in the seconds after a game ends.
    #[test]
    fn a_404_is_the_stats_not_being_written_yet() {
        assert!(status_is_transient(Some(404)));
        assert!(status_is_transient(Some(204)));
        assert!(status_is_transient(Some(503)));
    }

    /// Retrying these for a minute achieves nothing but load on a client
    /// the user is trying to play on.
    #[test]
    fn an_auth_failure_or_a_dead_client_stops_immediately() {
        assert!(!status_is_transient(Some(401)));
        assert!(!status_is_transient(Some(403)));
        assert!(!status_is_transient(Some(400)));
        assert!(!status_is_transient(None), "no response at all: the client is gone");
    }

    #[test]
    fn a_participant_we_cannot_find_yet_is_worth_another_look() {
        assert!(worth_retrying(&MatchDataError::ParticipantNotFound(555)));
        assert!(worth_retrying(&MatchDataError::NotReady(555)));
    }

    #[test]
    fn a_body_we_cannot_parse_is_never_going_to_parse() {
        let json = serde_json::from_str::<i32>("not a number").unwrap_err();
        assert!(!worth_retrying(&MatchDataError::Client(json.into())));
    }

    // --- summary → columns ------------------------------------------------

    /// The champion name is the one thing this patch must not write: the
    /// LCU gives an id, and the alias it resolves to (`MonkeyKing`) is not
    /// the display name Live Client Data wrote (`Wukong`).
    #[test]
    fn the_patch_never_writes_a_champion() {
        let meta = to_metadata(&MatchSummary {
            champion_id: Some(62),
            queue_id: Some(420),
            ..Default::default()
        });
        assert_eq!(meta.champion, None);
        assert_eq!(meta.queue, Some(420));
    }

    #[test]
    fn every_column_the_lcu_answers_for_is_carried_across() {
        let meta = to_metadata(&MatchSummary {
            game_id: Some(555),
            queue_id: Some(420),
            champion_id: Some(62),
            win: Some(true),
            kills: Some(7),
            deaths: Some(2),
            assists: Some(5),
            role: Some("Middle".into()),
            patch: Some("15.3.412.9873".into()),
        });

        assert_eq!(
            meta,
            MatchMetadata {
                game_id: Some(555),
                queue: Some(420),
                role: Some("Middle".into()),
                patch: Some("15.3.412.9873".into()),
                win: Some(true),
                kda_k: Some(7),
                kda_d: Some(2),
                kda_a: Some(5),
                champion: None,
            }
        );
    }

    // --- disagreement -----------------------------------------------------

    fn live(win: Option<bool>, kda: Option<Kda>) -> LiveSummary {
        LiveSummary {
            champion: Some("Ahri".into()),
            kda,
            game_mode: Some("CLASSIC".into()),
            win,
        }
    }

    #[test]
    fn two_views_of_the_same_game_agree_and_say_nothing() {
        let summary = MatchSummary {
            win: Some(true),
            kills: Some(7),
            deaths: Some(2),
            assists: Some(5),
            ..Default::default()
        };
        let kda = Kda {
            kills: 7,
            deaths: 2,
            assists: 5,
        };
        assert!(disagreements(&live(Some(true), Some(kda)), &summary).is_empty());
    }

    /// The signal that matters: the two sources cannot legitimately
    /// disagree about who won, so if they do, the wrong game was matched.
    #[test]
    fn a_contradicted_outcome_is_reported() {
        let summary = MatchSummary {
            win: Some(false),
            ..Default::default()
        };
        let notes = disagreements(&live(Some(true), None), &summary);
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("a win"), "{}", notes[0]);
        assert!(notes[0].contains("a loss"), "{}", notes[0]);
    }

    #[test]
    fn a_contradicted_kda_is_reported() {
        let summary = MatchSummary {
            kills: Some(1),
            deaths: Some(9),
            assists: Some(0),
            ..Default::default()
        };
        let kda = Kda {
            kills: 7,
            deaths: 2,
            assists: 5,
        };
        let notes = disagreements(&live(None, Some(kda)), &summary);
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("7/2/5"), "{}", notes[0]);
        assert!(notes[0].contains("1/9/0"), "{}", notes[0]);
    }

    /// A field only one side established is not a disagreement. Most
    /// recordings are this case — Practice Tool never reaches the LCU, and
    /// a game whose poller never came up has no live values at all.
    #[test]
    fn a_field_only_one_source_knows_is_not_a_disagreement() {
        let summary = MatchSummary {
            win: Some(true),
            kills: Some(7),
            ..Default::default()
        };
        assert!(disagreements(&live(None, None), &summary).is_empty());
        assert!(disagreements(&live(Some(true), None), &MatchSummary::default()).is_empty());
    }

    // --- the write --------------------------------------------------------

    /// End to end over the pure half plus the DB write, which is
    /// everything in this module except the HTTP call itself.
    #[test]
    fn a_patched_row_gains_the_lcu_columns_and_keeps_the_live_ones() {
        let db = Db::open_in_memory().unwrap();
        let id = db
            .insert_recording(&NewRecording {
                path: "/game.mp4".into(),
                started_at: 1,
                champion: Some("Ahri".into()),
                win: Some(true),
                pinned: true,
                ..Default::default()
            })
            .unwrap();

        let meta = to_metadata(&MatchSummary {
            game_id: Some(555),
            queue_id: Some(420),
            champion_id: Some(103),
            win: Some(true),
            role: Some("Middle".into()),
            patch: Some("15.3.412.9873".into()),
            ..Default::default()
        });
        assert_eq!(db.update_match_metadata(id, &meta).unwrap(), 1);

        let row = db.get_recording(id).unwrap().unwrap();
        assert_eq!(row.queue, Some(420));
        assert_eq!(row.role.as_deref(), Some("Middle"));
        assert_eq!(row.champion.as_deref(), Some("Ahri"));
        assert!(row.pinned);
    }
}
