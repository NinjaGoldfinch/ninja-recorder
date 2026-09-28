//! The fetch-once catch-up (#349): documents for recordings made before the
//! archive existed.
//!
//! Those recordings have no documents, so nothing can re-derive them. The
//! League client still has their match-history games, as far back as its
//! history reaches. So whenever the client connects, and only while no game is
//! on, the daemon fetches each one's documents by game id, a few seconds
//! apart, archives them, and re-derives. A game the client no longer has is
//! recorded as missing and asked about again only by a newer app version, not
//! on every connect. Nothing here has a button.

use crate::db::documents::{ArchiveOutcome, DocumentKind};
use crate::db::Db;
use crate::lcu;
use crate::match_summary::{archive_lcu_documents, Archived};
use crate::{debug, info, warn};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// The gap between two games' fetches. The client is a local process, but it
/// is also the thing the person is about to queue in: a catch-up of a hundred
/// games should be a trickle in the background, not a burst.
const GAP: Duration = Duration::from_secs(3);

/// One catch-up at a time. A second connect while one is running would only
/// fetch the same games twice.
static RUNNING: AtomicBool = AtomicBool::new(false);

/// What an attempt says about a recording. Pure.
///
/// A match document kept is `Archived`. The client answering 404 for the
/// game is `Missing`: it does not have it. Anything else is `Failed`, and is
/// retried on the next connect.
pub fn outcome(has_match_document: bool, archived: Archived) -> ArchiveOutcome {
    if has_match_document {
        ArchiveOutcome::Archived
    } else if archived.match_not_found {
        ArchiveOutcome::Missing
    } else {
        ArchiveOutcome::Failed
    }
}

/// Fetches documents for every recording that needs them, while `is_quiet`
/// says no game is on, then re-derives. Returns how many recordings the
/// re-derivation changed.
pub async fn run(
    db: &Arc<Db>,
    lockfile: &lcu::LockfileInfo,
    is_quiet: impl Fn() -> bool,
    app_version: &str,
    now_ms: impl Fn() -> i64,
) -> usize {
    if RUNNING.swap(true, Ordering::SeqCst) {
        debug!("catch-up", "already running");
        return 0;
    }
    let changed = catch_up(db, lockfile, is_quiet, app_version, now_ms).await;
    RUNNING.store(false, Ordering::SeqCst);
    changed
}

async fn catch_up(
    db: &Arc<Db>,
    lockfile: &lcu::LockfileInfo,
    is_quiet: impl Fn() -> bool,
    app_version: &str,
    now_ms: impl Fn() -> i64,
) -> usize {
    let todo = match db.recordings_to_archive(app_version) {
        Ok(todo) if !todo.is_empty() => todo,
        Ok(_) => return 0,
        Err(e) => {
            warn!("catch-up", "could not list recordings to archive: {e}");
            return 0;
        }
    };
    let client = match lcu::LcuHttpClient::new(lockfile) {
        Ok(client) => client,
        Err(e) => {
            warn!("catch-up", "no client to fetch from: {e}");
            return 0;
        }
    };
    info!("catch-up", "fetching documents for {} recording(s) from before the archive", todo.len());

    let mut archived = 0;
    for (i, (recording_id, game_id)) in todo.iter().enumerate() {
        if !is_quiet() {
            info!("catch-up", "a game is on; stopping after {i} of {}", todo.len());
            break;
        }
        if i > 0 {
            tokio::time::sleep(GAP).await;
        }
        let result = archive_lcu_documents(db, &client, lockfile, *recording_id, *game_id, now_ms()).await;
        let has_match = db.has_document(*recording_id, DocumentKind::Match).unwrap_or(false);
        let outcome = outcome(has_match, result);
        if outcome == ArchiveOutcome::Archived {
            archived += 1;
        }
        if let Err(e) = db.record_archive_attempt(*recording_id, app_version, outcome, now_ms()) {
            warn!("catch-up", "could not record the attempt for recording {recording_id}: {e}");
        }
    }
    info!("catch-up", "archived documents for {archived} recording(s)");

    if archived == 0 {
        return 0;
    }
    // Off the runtime, like the startup pass: database and CPU work.
    let db = Arc::clone(db);
    tokio::task::spawn_blocking(move || crate::derive::rederive_outdated(&db))
        .await
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kept_match_document_is_archived_whatever_else_happened() {
        let any = Archived { written: 1, match_not_found: true };
        assert_eq!(outcome(true, any), ArchiveOutcome::Archived);
    }

    /// A 404 is the client saying it does not have the game: asked about
    /// again only by a newer version.
    #[test]
    fn a_404_is_missing() {
        let archived = Archived { written: 1, match_not_found: true };
        assert_eq!(outcome(false, archived), ArchiveOutcome::Missing);
    }

    /// Anything else is not an answer about the game, and is retried.
    #[test]
    fn no_answer_is_a_failure_to_retry() {
        assert_eq!(outcome(false, Archived::default()), ArchiveOutcome::Failed);
        let summoner_only = Archived { written: 1, match_not_found: false };
        assert_eq!(outcome(false, summoner_only), ArchiveOutcome::Failed);
    }

    fn finished(db: &Db, path: &str, started_at: i64, game_id: Option<i64>) -> i64 {
        let id = db.begin_recording(path, started_at, None).unwrap();
        db.finish_recording(id, &crate::db::NewRecording {
            path: path.into(),
            started_at,
            finished_at: Some(started_at + 1),
            game_id,
            ..Default::default()
        })
        .unwrap();
        id
    }

    #[test]
    fn the_work_list_is_finished_recordings_with_a_game_and_no_match_document() {
        let db = Db::open_temporary().unwrap();
        let old = finished(&db, "C:/vods/a.mp4", 1000, Some(7));
        let newer = finished(&db, "C:/vods/b.mp4", 2000, Some(8));
        finished(&db, "C:/vods/imported.mp4", 3000, None);
        let done = finished(&db, "C:/vods/c.mp4", 4000, Some(9));
        db.put_document(done, DocumentKind::Match, "{}", 0).unwrap();
        db.begin_recording("C:/vods/open.mp4", 5000, None).unwrap();

        assert_eq!(db.recordings_to_archive("1.0").unwrap(), vec![(newer, 8), (old, 7)]);
    }

    /// Missing is remembered per version; failed is not remembered at all.
    #[test]
    fn a_missing_game_is_asked_about_again_only_by_a_newer_version() {
        let db = Db::open_temporary().unwrap();
        let missing = finished(&db, "C:/vods/a.mp4", 1000, Some(7));
        let failed = finished(&db, "C:/vods/b.mp4", 2000, Some(8));
        db.record_archive_attempt(missing, "1.0", ArchiveOutcome::Missing, 0).unwrap();
        db.record_archive_attempt(failed, "1.0", ArchiveOutcome::Failed, 0).unwrap();

        assert_eq!(db.recordings_to_archive("1.0").unwrap(), vec![(failed, 8)]);
        assert_eq!(db.recordings_to_archive("1.1").unwrap().len(), 2);
    }

    /// New documents make the derived board stale, so a board derived from
    /// the live snapshot alone is re-derived once the match document arrives.
    #[test]
    fn a_new_document_makes_the_scoreboard_stale() {
        let db = Db::open_temporary().unwrap();
        let id = finished(&db, "C:/vods/a.mp4", 1000, Some(7));
        db.write_derived_scoreboard(id, "{\"players\":[]}", None, crate::derive::SCOREBOARD_VERSION)
            .unwrap();
        assert!(db.recordings_to_rederive(crate::derive::SCOREBOARD_VERSION).unwrap().is_empty());

        db.put_document(id, DocumentKind::Match, "{}", 0).unwrap();
        assert_eq!(db.recordings_to_rederive(crate::derive::SCOREBOARD_VERSION).unwrap(), vec![id]);
    }
}
