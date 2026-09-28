//! Re-deriving a recording's data from its archived documents (#349).
//!
//! The documents are kept (`db::documents`); what the app shows is extracted
//! from them. **When an extraction changes, its version is bumped**, and every
//! recording an older version wrote is re-derived on the next start, from its
//! own documents, with no League client and no network. Nothing is ever
//! backfilled by hand.
//!
//! The scoreboard is the first derivation. It is derived exactly as the live
//! path builds it: the live snapshot's board, replaced by the match-history
//! board where there is one, which takes what only the live capture knew
//! through `match_summary::carry_live_fields`. The pieces are the ones the
//! live path runs, not copies of them.
//!
//! `scoreboard_output_is_pinned_to_its_version` is what makes "bump the
//! version" something CI enforces rather than something to remember.

use crate::db::documents::DocumentKind;
use crate::db::Db;
use crate::live_client::{self, AllGameData, Scoreboard};
use crate::match_summary::{board_player, carry_live_fields, runes_of};
use crate::{debug, info, warn};
use std::collections::HashMap;

/// The version of the scoreboard extraction. **Bump it whenever
/// `derive_scoreboard`'s output changes for the same documents**: every
/// recording a lower version wrote is then re-derived on the next start. The
/// golden test fails until it is bumped.
///
/// 1: the scoreboard as #346 left it (trinket, role item, every player's runes).
pub const SCOREBOARD_VERSION: i64 = 1;

/// The documents a scoreboard is derived from, as the text they were archived as.
#[derive(Debug, Default, Clone)]
pub struct Sources {
    pub live: Option<String>,
    pub match_doc: Option<String>,
    pub summoner: Option<String>,
}

#[derive(Debug, PartialEq)]
pub enum Derived {
    Board(Scoreboard),
    /// The match document names a champion by an id nothing can name yet.
    /// Not written: a board with a blank champion would be worse than the
    /// one already there. Retried on a later start, when the names are known.
    UnnamedChampion(i64),
    /// No document produced a board: a live snapshot with no players and no
    /// match document, say.
    Nothing,
}

/// A recording's scoreboard from its documents. Pure.
///
/// `previous` is the board already stored, the live half's stand-in when the
/// live snapshot was never archived (every recording from before #349).
/// `names` is the champion table the match document's ids resolve through.
pub fn derive_scoreboard(
    sources: &Sources,
    previous: Option<&Scoreboard>,
    names: &HashMap<i64, String>,
) -> Derived {
    let live_board = sources
        .live
        .as_deref()
        .and_then(|text| serde_json::from_str::<AllGameData>(text).ok())
        .and_then(|snapshot| live_client::scoreboard(&snapshot));

    let lcu_participants = match (sources.summoner.as_deref(), sources.match_doc.as_deref()) {
        (Some(summoner), Some(game)) => crate::lcu::participants_from_documents(summoner, game)
            .ok()
            .filter(|p| !p.is_empty()),
        _ => None,
    };

    let Some(participants) = lcu_participants else {
        return live_board.map_or(Derived::Nothing, Derived::Board);
    };

    // What the live capture knew: its own snapshot, else the stored board.
    let known = live_board.as_ref().or(previous);
    let mut players = Vec::with_capacity(participants.len());
    for p in &participants {
        let name = names
            .get(&p.champion_id)
            .cloned()
            .or_else(|| known.and_then(|board| name_by_line(board, p)));
        let Some(name) = name else {
            return Derived::UnnamedChampion(p.champion_id);
        };
        players.push(board_player(p, name));
    }
    if let Some(known) = known {
        carry_live_fields(&mut players, &known.players);
    }

    let us = participants.iter().find(|p| p.is_us);
    Derived::Board(Scoreboard {
        our_team: us.and_then(|p| p.team.clone()),
        our_runes: us.and_then(runes_of),
        players,
    })
}

/// A champion's name from a board that has it, found by the player's team
/// and K/D/A, which the match document and the live capture both hold. Only
/// a unique match counts.
fn name_by_line(board: &Scoreboard, p: &crate::lcu::ParticipantSummary) -> Option<String> {
    let team = p.team.as_deref()?;
    let mut matches = board.players.iter().filter(|b| {
        b.team == team && (b.kills, b.deaths, b.assists) == (p.kills, p.deaths, p.assists)
    });
    match (matches.next(), matches.next()) {
        (Some(one), None) if !one.champion.is_empty() => Some(one.champion.clone()),
        _ => None,
    }
}

/// Re-derives every finished recording an older extraction wrote, from its
/// archived documents. Returns how many rows changed. Run at daemon start,
/// off the async runtime: it is database and CPU work, a few milliseconds a
/// recording.
pub fn rederive_outdated(db: &Db) -> usize {
    let ids = match db.recordings_to_rederive(SCOREBOARD_VERSION) {
        Ok(ids) => ids,
        Err(e) => {
            warn!("derive", "could not list recordings to re-derive: {e}");
            return 0;
        }
    };
    if ids.is_empty() {
        return 0;
    }
    let names = db.champion_names().unwrap_or_default();
    let mut changed = 0;
    for id in &ids {
        match rederive_one(db, *id, &names) {
            Ok(true) => changed += 1,
            Ok(false) => {}
            Err(e) => warn!("derive", "could not re-derive recording {id}: {e}"),
        }
    }
    info!(
        "derive",
        "re-derived {changed} of {} recording(s) to scoreboard version {SCOREBOARD_VERSION}",
        ids.len()
    );
    changed
}

fn rederive_one(db: &Db, id: i64, names: &HashMap<i64, String>) -> Result<bool, crate::db::DbError> {
    let sources = Sources {
        live: db.get_document(id, DocumentKind::Live)?,
        match_doc: db.get_document(id, DocumentKind::Match)?,
        summoner: db.get_document(id, DocumentKind::Summoner)?,
    };
    let previous = db
        .get_recording(id)?
        .and_then(|row| row.scoreboard_json)
        .and_then(|json| serde_json::from_str::<Scoreboard>(&json).ok());
    match derive_scoreboard(&sources, previous.as_ref(), names) {
        Derived::Board(board) => {
            let cs = board.players.iter().find(|p| p.is_us).map(|p| p.cs);
            let json = serde_json::to_string(&board).expect("a scoreboard serialises");
            db.write_derived_scoreboard(id, &json, cs, SCOREBOARD_VERSION)
        }
        Derived::UnnamedChampion(champion) => {
            debug!("derive", "recording {id} waits for a name for champion {champion}");
            Ok(false)
        }
        Derived::Nothing => {
            debug!("derive", "recording {id} has no document that makes a board");
            Ok(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIVE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../fixtures/live-client/allgamedata-paired.json"
    ));
    const MATCH: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../fixtures/lcu/match-history-paired.json"
    ));
    /// Participant 2's synthetic identity in the paired match document.
    const SUMMONER: &str = r#"{"puuid": "00000000-0000-5000-8000-000000000002", "summonerId": 22222222}"#;

    fn names() -> HashMap<i64, String> {
        [
            (106, "Volibear"), (234, "Viego"), (910, "Hwei"), (429, "Kalista"), (223, "Tahm Kench"),
            (48, "Trundle"), (246, "Qiyana"), (80, "Pantheon"), (29, "Twitch"), (26, "Zilean"),
        ]
        .into_iter()
        .map(|(id, n)| (id, n.to_string()))
        .collect()
    }

    fn all_sources() -> Sources {
        Sources {
            live: Some(LIVE.into()),
            match_doc: Some(MATCH.into()),
            summoner: Some(SUMMONER.into()),
        }
    }

    fn board(derived: Derived) -> Scoreboard {
        match derived {
            Derived::Board(b) => b,
            other => panic!("expected a board, got {other:?}"),
        }
    }

    /// The match board wins, and keeps what only the live capture had: the
    /// positions, which match history's inference got wrong in this game.
    #[test]
    fn the_match_board_wins_and_keeps_the_live_positions() {
        let b = board(derive_scoreboard(&all_sources(), None, &names()));
        let volibear = b.players.iter().find(|p| p.champion == "Volibear").unwrap();
        // timeline.lane said JUNGLE; the live capture said TOP.
        assert_eq!(volibear.position.as_deref(), Some("Top"));
        // Match-history-only data is there too.
        let kalista = b.players.iter().find(|p| p.champion == "Kalista").unwrap();
        assert_eq!(kalista.role_item, Some(3008));
        let us = b.players.iter().find(|p| p.is_us).unwrap();
        assert_eq!(us.champion, "Viego");
        assert!(b.players.iter().all(|p| p.runes.is_some()));
    }

    #[test]
    fn a_live_snapshot_alone_derives_the_live_board() {
        let sources = Sources { live: Some(LIVE.into()), ..Default::default() };
        let b = board(derive_scoreboard(&sources, None, &HashMap::new()));
        let expected = live_client::scoreboard(&serde_json::from_str(LIVE).unwrap()).unwrap();
        assert_eq!(b, expected);
    }

    /// Champions the table cannot name are named from the live board by
    /// team and K/D/A, which both documents hold.
    #[test]
    fn a_champion_the_table_lacks_is_named_from_the_live_board() {
        let b = board(derive_scoreboard(&all_sources(), None, &HashMap::new()));
        assert_eq!(b.players.len(), 10);
        assert!(b.players.iter().all(|p| !p.champion.is_empty()));
    }

    /// With nothing to name a champion from, nothing is written: a blank
    /// champion is worse than the board already stored.
    #[test]
    fn an_unnameable_champion_writes_nothing() {
        let sources = Sources { live: None, ..all_sources() };
        assert!(matches!(
            derive_scoreboard(&sources, None, &HashMap::new()),
            Derived::UnnamedChampion(_)
        ));
    }

    #[test]
    fn no_documents_derive_nothing() {
        assert_eq!(derive_scoreboard(&Sources::default(), None, &names()), Derived::Nothing);
    }

    // --- the golden test ----------------------------------------------------

    const GOLDEN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../fixtures/derived/scoreboard.golden.json");

    #[derive(serde::Serialize, serde::Deserialize)]
    struct Golden {
        version: i64,
        scoreboard: Scoreboard,
    }

    /// **The scoreboard extraction's output is pinned to its version.** If a
    /// change alters what the paired game derives to without bumping
    /// `SCOREBOARD_VERSION`, recordings already written would never be
    /// re-derived, and old rows would silently disagree with new ones. So:
    ///
    /// - output unchanged: the golden file's version must equal the constant;
    /// - output changed, version not bumped: fail, and say to bump it;
    /// - output changed, version bumped: fail until the golden file is
    ///   rewritten, with `UPDATE_GOLDEN=1 cargo test`, which refuses to
    ///   rewrite it unless the version was bumped.
    #[test]
    fn scoreboard_output_is_pinned_to_its_version() {
        let derived = board(derive_scoreboard(&all_sources(), None, &names()));
        let stored: Option<Golden> =
            std::fs::read_to_string(GOLDEN).ok().and_then(|t| serde_json::from_str(&t).ok());

        if std::env::var_os("UPDATE_GOLDEN").is_some() {
            if let Some(stored) = &stored {
                assert!(
                    stored.scoreboard == derived || SCOREBOARD_VERSION > stored.version,
                    "the output changed: bump SCOREBOARD_VERSION before rewriting the golden file"
                );
            }
            let golden = Golden { version: SCOREBOARD_VERSION, scoreboard: derived };
            std::fs::create_dir_all(std::path::Path::new(GOLDEN).parent().unwrap()).unwrap();
            std::fs::write(GOLDEN, serde_json::to_string_pretty(&golden).unwrap() + "\n").unwrap();
            return;
        }

        let stored = stored.expect("no golden file: run `UPDATE_GOLDEN=1 cargo test` once");
        if stored.scoreboard != derived {
            assert!(
                SCOREBOARD_VERSION > stored.version,
                "the scoreboard extraction's output changed but SCOREBOARD_VERSION is still \
                 {SCOREBOARD_VERSION}. Bump it, so every recording an older version wrote is \
                 re-derived, then rewrite the golden file with `UPDATE_GOLDEN=1 cargo test`."
            );
            panic!(
                "SCOREBOARD_VERSION is bumped; now rewrite the golden file with \
                 `UPDATE_GOLDEN=1 cargo test`"
            );
        }
        assert_eq!(
            stored.version, SCOREBOARD_VERSION,
            "SCOREBOARD_VERSION changed but the output did not; rewrite the golden file"
        );
    }

    // --- the job, through the database ----------------------------------------

    fn finished(db: &Db) -> i64 {
        let id = db.begin_recording("C:/vods/a.mp4", 1000, None).unwrap();
        db.finish_recording(id, &crate::db::NewRecording {
            path: "C:/vods/a.mp4".into(),
            started_at: 1000,
            finished_at: Some(2000),
            ..Default::default()
        })
        .unwrap();
        id
    }

    #[test]
    fn outdated_recordings_are_re_derived_once() {
        let db = Db::open_temporary().unwrap();
        let id = finished(&db);
        db.put_document(id, DocumentKind::Live, LIVE, 0).unwrap();
        db.put_document(id, DocumentKind::Match, MATCH, 0).unwrap();
        db.put_document(id, DocumentKind::Summoner, SUMMONER, 0).unwrap();
        db.put_champion_names(&names()).unwrap();

        assert_eq!(rederive_outdated(&db), 1);
        let row = db.get_recording(id).unwrap().unwrap();
        let stored: Scoreboard = serde_json::from_str(row.scoreboard_json.as_deref().unwrap()).unwrap();
        assert_eq!(stored, board(derive_scoreboard(&all_sources(), None, &names())));
        // Match history's settled 11 + 186, not the last live poll's 190.
        assert_eq!(row.cs, Some(197), "our CS follows the board");

        // Current now, so a second start does nothing.
        assert_eq!(rederive_outdated(&db), 0);
    }

    /// A recording with no documents is left for the fetch-once catch-up,
    /// and one still in progress is never touched.
    #[test]
    fn only_finished_recordings_with_documents_are_candidates() {
        let db = Db::open_temporary().unwrap();
        finished(&db);
        let open = db.begin_recording("C:/vods/b.mp4", 2000, None).unwrap();
        db.put_document(open, DocumentKind::Live, LIVE, 0).unwrap();
        assert!(db.recordings_to_rederive(SCOREBOARD_VERSION).unwrap().is_empty());
    }
}
