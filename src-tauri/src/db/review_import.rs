//! The spreadsheet importer's database half (WS9 §3.2).
//!
//! The CSV itself is parsed in the webview (`src/lib/reviewform/sheet.ts`),
//! because that is where the file is chosen and where the local timezone,
//! DST included, turns a spreadsheet's date and time into a moment. What
//! arrives here is typed rows. This half decides what each row becomes, and
//! it is **idempotent**: importing the same rows twice changes nothing.
//!
//! - A row matches the game nearest its start within `MATCH_WINDOW_MS`. With
//!   no game there, it matches a recording that has none yet (anything
//!   recorded before WS9) and makes that recording's game. Otherwise it
//!   becomes a game with no recording.
//! - What a matched game or review already has wins: the import only fills
//!   what is empty, so a review edited in the app survives a re-import.
//! - Objectives are deduplicated by `normalise`d text, against the ones that
//!   already exist as well as each other.
//! - Takeaways are deduplicated by exact text per game, or per block.
//! - Rows sharing a spreadsheet `block` label end up in one block.

use super::review::{assign_block, ensure_game_in, GameResult, LaneRating, MentalRating};
use super::{Db, DbError};
use rusqlite::{params, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

/// How far a spreadsheet row's start may be from a game's and still be it.
pub const MATCH_WINDOW_MS: i64 = 5 * 60 * 1000;

/// One spreadsheet row, as the webview parsed it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
pub struct ImportRow {
    /// The CSV line the row came from, for the report.
    pub line: i64,
    /// Unix millis, from the `date` and `time` columns in local time.
    pub started_at: i64,
    /// The `block` column, as written. Rows sharing it share a block.
    pub block: Option<String>,
    pub champion: Option<String>,
    pub matchup: Option<String>,
    pub game: Option<GameResult>,
    pub lane: Option<LaneRating>,
    pub mental: Option<MentalRating>,
    pub clear_ms: Option<i64>,
    pub smites: Option<i64>,
    pub deaths: Option<i64>,
    pub objectives: Vec<String>,
    pub takeaways: Vec<String>,
    pub block_takeaways: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
pub struct ImportReport {
    pub rows: i64,
    pub games_created: i64,
    pub games_matched: i64,
    pub objectives_created: i64,
    pub takeaways_created: i64,
    pub blocks_merged: i64,
}

/// The text two objectives are compared by: case, spacing, bullet glyphs
/// and trailing punctuation do not make a different objective.
pub fn normalise(text: &str) -> String {
    let trimmed = text
        .trim()
        .trim_start_matches(['•', '-', '*', '·', '–'])
        .trim()
        .trim_end_matches(['.', '!', ';', ','])
        .trim();
    trimmed.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

/// The game a row is: the nearest start within the window, earlier on a tie.
pub fn nearest_game(candidates: &[(i64, i64)], started_at: i64) -> Option<i64> {
    candidates
        .iter()
        .map(|&(id, at)| ((at - started_at).abs(), at, id))
        .filter(|&(distance, _, _)| distance <= MATCH_WINDOW_MS)
        .min()
        .map(|(_, _, id)| id)
}

fn clean(texts: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in texts {
        let t = t.trim();
        if !t.is_empty() && !out.iter().any(|o| o == t) {
            out.push(t.to_string());
        }
    }
    out
}

fn find_or_create_game(tx: &Transaction, row: &ImportRow) -> Result<(i64, bool), DbError> {
    let candidates = tx
        .prepare("SELECT id, started_at FROM games WHERE started_at BETWEEN ?1 - ?2 AND ?1 + ?2")?
        .query_map(params![row.started_at, MATCH_WINDOW_MS], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<Vec<(i64, i64)>, _>>()?;
    let matched = match nearest_game(&candidates, row.started_at) {
        Some(id) => Some(id),
        // A recording from before WS9 has no game yet. Making it here, rather
        // than a game with no recording, is what stops opening that
        // recording's review later from making a second one.
        None => {
            let recordings = tx
                .prepare(
                    "SELECT r.id, r.started_at FROM recordings r
                     WHERE r.finished_at IS NOT NULL
                       AND r.started_at BETWEEN ?1 - ?2 AND ?1 + ?2
                       AND NOT EXISTS (SELECT 1 FROM games g WHERE g.recording_id = r.id)",
                )?
                .query_map(params![row.started_at, MATCH_WINDOW_MS], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<Result<Vec<(i64, i64)>, _>>()?;
            match nearest_game(&recordings, row.started_at) {
                Some(recording_id) => Some(ensure_game_in(tx, recording_id)?),
                None => None,
            }
        }
    };
    if let Some(id) = matched {
        tx.execute(
            "UPDATE games SET champion = COALESCE(champion, ?2), matchup = COALESCE(matchup, ?3),
                              result = COALESCE(result, ?4)
             WHERE id = ?1",
            params![id, row.champion, row.matchup, row.game],
        )?;
        return Ok((id, false));
    }
    tx.execute(
        "INSERT INTO games (started_at, champion, matchup, result) VALUES (?1, ?2, ?3, ?4)",
        params![row.started_at, row.champion, row.matchup, row.game],
    )?;
    let id = tx.last_insert_rowid();
    assign_block(tx, id, row.started_at)?;
    Ok((id, true))
}

fn fill_review(tx: &Transaction, game_id: i64, row: &ImportRow) -> Result<(), DbError> {
    tx.execute("INSERT OR IGNORE INTO game_reviews (game_id) VALUES (?1)", [game_id])?;
    tx.execute(
        "UPDATE game_reviews SET
            game_rating = COALESCE(game_rating, ?2),
            lane_rating = COALESCE(lane_rating, ?3),
            mental_rating = COALESCE(mental_rating, ?4),
            first_clear_ms = COALESCE(first_clear_ms, ?5),
            smites_at_clear = COALESCE(smites_at_clear, ?6),
            deaths = COALESCE(deaths, ?7)
         WHERE game_id = ?1",
        params![game_id, row.game, row.lane, row.mental, row.clear_ms, row.smites, row.deaths],
    )?;
    Ok(())
}

/// Adds a takeaway unless the same text is already there for that owner.
fn add_takeaway_once(
    tx: &Transaction,
    game_id: Option<i64>,
    block_id: Option<i64>,
    body: &str,
    created_at: i64,
) -> Result<bool, DbError> {
    let exists: bool = tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM takeaways
                        WHERE game_id IS ?1 AND block_id IS ?2 AND body = ?3)",
        params![game_id, block_id, body],
        |r| r.get(0),
    )?;
    if exists {
        return Ok(false);
    }
    tx.execute(
        "INSERT INTO takeaways (game_id, block_id, body, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![game_id, block_id, body, created_at],
    )?;
    Ok(true)
}

impl Db {
    /// Imports spreadsheet rows in one transaction. See the module docs for
    /// what each row becomes and why a second run is a no-op.
    pub fn import_review_rows(&self, rows: &[ImportRow]) -> Result<ImportReport, DbError> {
        let mut report = ImportReport { rows: rows.len() as i64, ..Default::default() };
        let mut conn = self.pool.write();
        let tx = conn.transaction()?;

        // Every objective that exists, by the text it is compared by.
        let mut objective_ids: HashMap<String, i64> = tx
            .prepare("SELECT id, body FROM objectives")?
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
            .map(|r| r.map(|(id, body)| (normalise(&body), id)))
            .collect::<Result<_, _>>()?;
        // The objectives the latest row lists are the ones still active; any
        // other objective this import creates was retired as of the last
        // game that listed it.
        let latest = rows.iter().max_by_key(|r| r.started_at);
        let current: Vec<String> = latest
            .map(|r| r.objectives.iter().map(|o| normalise(o)).collect())
            .unwrap_or_default();
        let mut last_seen: HashMap<i64, i64> = HashMap::new();
        let mut created_objectives: Vec<(i64, String)> = Vec::new();
        let mut by_label: BTreeMap<String, Vec<i64>> = BTreeMap::new();
        let mut block_takeaways: BTreeMap<String, (Vec<String>, i64)> = BTreeMap::new();

        let mut ordered: Vec<&ImportRow> = rows.iter().collect();
        ordered.sort_by_key(|r| (r.started_at, r.line));

        for row in ordered {
            let (game_id, created) = find_or_create_game(&tx, row)?;
            if created {
                report.games_created += 1;
            } else {
                report.games_matched += 1;
            }
            fill_review(&tx, game_id, row)?;

            for body in clean(&row.objectives) {
                let key = normalise(&body);
                let objective_id = match objective_ids.get(&key) {
                    Some(&id) => id,
                    None => {
                        tx.execute(
                            "INSERT INTO objectives (body, created_at) VALUES (?1, ?2)",
                            params![body, row.started_at],
                        )?;
                        let id = tx.last_insert_rowid();
                        objective_ids.insert(key.clone(), id);
                        created_objectives.push((id, key));
                        report.objectives_created += 1;
                        id
                    }
                };
                last_seen.insert(objective_id, row.started_at);
                tx.execute(
                    "INSERT OR IGNORE INTO game_objectives (game_id, objective_id) VALUES (?1, ?2)",
                    [game_id, objective_id],
                )?;
            }

            for body in clean(&row.takeaways) {
                if add_takeaway_once(&tx, Some(game_id), None, &body, row.started_at)? {
                    report.takeaways_created += 1;
                }
            }

            if let Some(label) = row.block.as_deref().map(str::trim).filter(|l| !l.is_empty()) {
                by_label.entry(label.to_string()).or_default().push(game_id);
                let entry = block_takeaways
                    .entry(label.to_string())
                    .or_insert_with(|| (Vec::new(), row.started_at));
                for body in clean(&row.block_takeaways) {
                    if !entry.0.contains(&body) {
                        entry.0.push(body);
                    }
                }
            }
        }

        for (id, key) in created_objectives {
            let (status, retired_at) = if current.contains(&key) {
                ("active", None)
            } else {
                ("retired", last_seen.get(&id).copied())
            };
            tx.execute(
                "UPDATE objectives SET status = ?2, retired_at = ?3 WHERE id = ?1",
                params![id, status, retired_at],
            )?;
        }

        // One block per label: the spreadsheet is the user's own account of
        // their sessions, so it wins over the gap rule.
        for (label, games) in &by_label {
            let blocks: Vec<i64> = {
                let mut seen: Vec<i64> = Vec::new();
                for g in games {
                    let block: Option<i64> = tx
                        .query_row("SELECT block_id FROM games WHERE id = ?1", [g], |r| r.get(0))
                        .optional()?
                        .flatten();
                    if let Some(b) = block
                        && !seen.contains(&b)
                    {
                        seen.push(b);
                    }
                }
                seen
            };
            let Some((&into, rest)) = blocks.split_first() else { continue };
            for &from in rest {
                super::review::merge_blocks_in(&tx, into, from)?;
                report.blocks_merged += 1;
            }
            if let Some((bodies, at)) = block_takeaways.get(label) {
                for body in bodies {
                    if add_takeaway_once(&tx, None, Some(into), body, *at)? {
                        report.takeaways_created += 1;
                    }
                }
            }
        }

        tx.commit()?;
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::super::review::{GameFacts, ObjectiveStatus, ReviewInput};
    use super::super::NewRecording;
    use super::*;

    const MIN: i64 = 60 * 1000;
    const HOUR: i64 = 60 * MIN;

    fn row(line: i64, started_at: i64, block: &str) -> ImportRow {
        ImportRow {
            line,
            started_at,
            block: Some(block.into()),
            champion: Some("Lee Sin".into()),
            matchup: Some("Vi".into()),
            game: Some(GameResult::Loss),
            lane: Some(LaneRating::Neutral),
            mental: Some(MentalRating::Good),
            clear_ms: Some(178_000),
            smites: Some(1),
            deaths: Some(7),
            objectives: vec!["Ward river at 2:45".into(), "Track the enemy jungler".into()],
            takeaways: vec![format!("takeaway from line {line}")],
            block_takeaways: vec!["Stop after two losses".into()],
        }
    }

    /// Three games over two sessions, with the objectives changing between
    /// them the way the spreadsheet's do.
    fn sheet() -> Vec<ImportRow> {
        let day2 = 24 * HOUR;
        let mut rows = vec![row(2, 0, "1"), row(3, 40 * MIN, "1"), row(4, day2, "2")];
        rows[2].objectives = vec!["  • ward RIVER at 2:45. ".into(), "Hold wave before recall".into()];
        rows[2].block_takeaways = vec![];
        rows
    }

    fn count(db: &Db, sql: &str) -> i64 {
        db.pool.read().query_row(sql, [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn objectives_compare_by_normalised_text() {
        assert_eq!(normalise("  • Ward RIVER   at 2:45. "), "ward river at 2:45");
        assert_eq!(normalise("- Track the jungler"), normalise("track the jungler"));
        assert_ne!(normalise("ward river"), normalise("ward tri"));
    }

    #[test]
    fn a_row_matches_the_nearest_game_within_five_minutes() {
        let games = [(1, 0), (2, 4 * MIN), (3, 20 * MIN)];
        assert_eq!(nearest_game(&games, 3 * MIN), Some(2));
        assert_eq!(nearest_game(&games, 2 * MIN), Some(1), "a tie goes to the earlier");
        assert_eq!(nearest_game(&games, 26 * MIN), None);
    }

    #[test]
    fn a_sheet_becomes_games_reviews_objectives_takeaways_and_blocks() {
        let db = Db::open_temporary().unwrap();
        let report = db.import_review_rows(&sheet()).unwrap();

        assert_eq!(report.rows, 3);
        assert_eq!(report.games_created, 3);
        assert_eq!(report.objectives_created, 3, "the reworded ward objective is not a new one");
        assert_eq!(report.takeaways_created, 3 + 1, "three game takeaways, one block takeaway");
        assert_eq!(count(&db, "SELECT COUNT(*) FROM game_reviews WHERE deaths = 7 AND lane_rating = 'neutral'"), 3);
        assert_eq!(count(&db, "SELECT COUNT(*) FROM game_objectives"), 6);
        assert_eq!(count(&db, "SELECT COUNT(DISTINCT block_id) FROM games"), 2);
        assert_eq!(
            count(&db, "SELECT COUNT(*) FROM takeaways WHERE block_id IS NOT NULL AND body = 'Stop after two losses'"),
            1,
            "a block takeaway repeated on each row of the block is one takeaway"
        );
    }

    #[test]
    fn the_latest_rows_objectives_are_active_and_the_rest_retired() {
        let db = Db::open_temporary().unwrap();
        db.import_review_rows(&sheet()).unwrap();

        let active: Vec<String> = db
            .list_objectives(Some(ObjectiveStatus::Active))
            .unwrap()
            .into_iter()
            .map(|o| o.body)
            .collect();
        assert_eq!(active.len(), 2);
        assert!(active.contains(&"Ward river at 2:45".to_string()));
        let retired = db.list_objectives(Some(ObjectiveStatus::Retired)).unwrap();
        assert_eq!(retired.len(), 1);
        assert_eq!(retired[0].body, "Track the enemy jungler");
        assert_eq!(retired[0].retired_at, Some(40 * MIN), "retired as of the last game that listed it");
    }

    #[test]
    fn importing_the_same_sheet_twice_changes_nothing() {
        let db = Db::open_temporary().unwrap();
        db.import_review_rows(&sheet()).unwrap();
        let tables = ["games", "blocks", "game_reviews", "objectives", "game_objectives", "takeaways"];
        let before: Vec<i64> = tables.iter().map(|t| count(&db, &format!("SELECT COUNT(*) FROM {t}"))).collect();

        let again = db.import_review_rows(&sheet()).unwrap();

        let after: Vec<i64> = tables.iter().map(|t| count(&db, &format!("SELECT COUNT(*) FROM {t}"))).collect();
        assert_eq!(before, after);
        assert_eq!(
            (again.games_created, again.games_matched, again.objectives_created, again.takeaways_created, again.blocks_merged),
            (0, 3, 0, 0, 0)
        );
    }

    /// The point of matching: a game the app recorded gets the spreadsheet's
    /// review rather than a duplicate beside it, and what the app already
    /// knows is not overwritten.
    #[test]
    fn a_row_fills_in_a_recorded_game_without_overwriting_it() {
        let db = Db::open_temporary().unwrap();
        let recording = db.begin_recording("C:/vods/a.mp4", 3 * MIN).unwrap();
        let game = db.start_game(Some(recording), 3 * MIN).unwrap();
        db.finish_game(game, &GameFacts {
            champion: Some("Viego".into()),
            ..Default::default()
        })
        .unwrap();
        db.upsert_review(game, &ReviewInput {
            mental_rating: Some(MentalRating::Bad),
            ..Default::default()
        })
        .unwrap();

        let report = db.import_review_rows(&[row(2, 0, "1")]).unwrap();

        assert_eq!((report.games_created, report.games_matched), (0, 1));
        let review = db.get_game_review(game).unwrap().unwrap();
        assert_eq!(review.game.recording_id, Some(recording));
        let saved = review.review.unwrap();
        assert_eq!(saved.mental_rating, Some(MentalRating::Bad), "the app's answer wins");
        assert_eq!(saved.deaths, Some(7), "an empty field is filled");
        assert_eq!(count(&db, "SELECT COUNT(*) FROM games WHERE champion = 'Viego'"), 1);
    }

    /// A recording from before WS9 has no game. The row makes its game,
    /// rather than a second, unrecorded one beside it.
    #[test]
    fn a_row_matches_a_recording_that_has_no_game_yet() {
        let db = Db::open_temporary().unwrap();
        let recording = db
            .insert_recording(&NewRecording {
                path: "C:/vods/old.mp4".into(),
                started_at: 2 * MIN,
                champion: Some("Ahri".into()),
                finished_at: Some(2 * MIN),
                ..Default::default()
            })
            .unwrap();

        let report = db.import_review_rows(&[row(2, 0, "1")]).unwrap();

        assert_eq!((report.games_created, report.games_matched), (0, 1));
        assert_eq!(count(&db, "SELECT COUNT(*) FROM games"), 1);
        let game = db.ensure_game_for_recording(recording).unwrap();
        let review = db.get_game_review(game).unwrap().unwrap();
        assert_eq!(review.game.champion.as_deref(), Some("Ahri"), "the recording's own facts win");
        assert_eq!(review.review.unwrap().deaths, Some(7));
    }

    /// The gap rule would split these (three hours apart), but the sheet
    /// says they were one session, and the sheet is the user's own record.
    #[test]
    fn rows_sharing_a_block_label_share_a_block() {
        let db = Db::open_temporary().unwrap();
        let report = db.import_review_rows(&[row(2, 0, "7"), row(3, 3 * HOUR, "7")]).unwrap();
        assert_eq!(report.blocks_merged, 1);
        assert_eq!(count(&db, "SELECT COUNT(*) FROM blocks"), 1);
        assert_eq!(count(&db, "SELECT COUNT(DISTINCT block_id) FROM games"), 1);
    }
}
