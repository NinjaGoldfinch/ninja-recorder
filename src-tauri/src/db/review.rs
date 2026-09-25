//! VOD review (WS9): games, blocks, reviews, objectives and takeaways.
//!
//! The tables are migration 13 in `db/mod.rs`; `docs/data-model.md` has the
//! diagram. A review hangs off `games` rather than `recordings`, because a
//! recording row is deleted with its file and a review has to outlive it.
//!
//! The one decision here is which block a game belongs to, and it is pure:
//! `choose_block` takes the candidates and a time and returns an answer, so
//! the 2-hour rule is tested without a database. Nothing in this file reads
//! the clock. Every method that stamps a time takes `now` from its caller.

use super::{Db, DbError};
use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSql, ToSqlOutput, ValueRef};
use rusqlite::{params, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};

/// A game that starts more than this long after its block's last game ended
/// opens a new block. Provisional (#250, DEVELOPMENT.md §20).
pub const BLOCK_GAP_MS: i64 = 2 * 60 * 60 * 1000;

/// A string enum stored as TEXT under a CHECK constraint. The spellings here
/// and in migration 13 are the same list; `every_enum_round_trips_through_sqlite`
/// fails if they drift.
macro_rules! text_enum {
    ($(#[$meta:meta])* $name:ident { $($variant:ident = $text:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
        #[serde(rename_all = "lowercase")]
        pub enum $name { $($variant),+ }

        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$variant),+];

            pub fn as_str(self) -> &'static str {
                match self { $($name::$variant => $text),+ }
            }

            pub fn parse(text: &str) -> Option<Self> {
                match text { $($text => Some($name::$variant),)+ _ => None }
            }
        }

        impl ToSql for $name {
            fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
                Ok(ToSqlOutput::from(self.as_str()))
            }
        }

        impl FromSql for $name {
            fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
                let text = value.as_str()?;
                $name::parse(text).ok_or_else(|| FromSqlError::Other(
                    format!("{text:?} is not a {}", stringify!($name)).into(),
                ))
            }
        }
    };
}

text_enum! {
    /// How the game went, as a result and as the review's own rating.
    GameResult { Win = "win", Loss = "loss" }
}
text_enum! {
    /// How the lane went.
    LaneRating { Win = "win", Neutral = "neutral", Loss = "loss" }
}
text_enum! {
    /// How the player's head was.
    MentalRating { Good = "good", Neutral = "neutral", Bad = "bad" }
}
text_enum! {
    ObjectiveStatus { Active = "active", Paused = "paused", Retired = "retired" }
}
text_enum! {
    ObjectiveCategory {
        Macro = "macro", Lane = "lane", Mental = "mental", Mechanics = "mechanics", Other = "other",
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
pub struct Objective {
    pub id: i64,
    pub body: String,
    pub category: ObjectiveCategory,
    pub status: ObjectiveStatus,
    pub created_at: i64,
    pub retired_at: Option<i64>,
}

/// One objective as a game was reviewed against it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
pub struct GameObjective {
    pub objective_id: i64,
    pub body: String,
    pub category: ObjectiveCategory,
    pub status: ObjectiveStatus,
    pub ticked: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
pub struct Takeaway {
    pub id: i64,
    pub game_id: Option<i64>,
    pub block_id: Option<i64>,
    pub body: String,
    pub objective_id: Option<i64>,
    pub promoted_to_id: Option<i64>,
    pub created_at: i64,
}

/// Whose takeaway it is. Exactly one, which is what the table's CHECK says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(tag = "kind", content = "id", rename_all = "lowercase")]
pub enum TakeawayOwner {
    Game(i64),
    Block(i64),
}

/// Everything the review form edits in one save. The form sends the whole
/// thing each time rather than a field at a time, so an autosave that lands
/// late cannot interleave with another and leave half of each.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
pub struct ReviewInput {
    pub game_rating: Option<GameResult>,
    pub lane_rating: Option<LaneRating>,
    pub mental_rating: Option<MentalRating>,
    pub first_clear_ms: Option<i64>,
    pub smites_at_clear: Option<i64>,
    /// `None` means "use the death markers", not zero.
    pub deaths: Option<i64>,
    pub free_notes: String,
}

/// The game as the review form's header shows it.
///
/// While the recording exists its champion and result win over the game's
/// own copies: the LCU corrects both on the recording after finalize, and
/// the copy on `games` is what is left once the VOD is gone.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
pub struct GameSummary {
    pub id: i64,
    pub recording_id: Option<i64>,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub block_id: Option<i64>,
    pub champion: Option<String>,
    /// The lane opponent's champion, or `None` where no board says who.
    pub matchup: Option<String>,
    pub result: Option<GameResult>,
}

/// Everything the review form shows for one game.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
pub struct GameReview {
    pub game: GameSummary,
    /// `None` until the first save.
    pub review: Option<ReviewInput>,
    /// Death markers on the recording: the value `deaths` shows when unset.
    /// `None` where there is nothing to count, which is not the same as zero:
    /// no recording, or a recording the poller never saw.
    pub death_markers: Option<i64>,
    pub objectives: Vec<GameObjective>,
    pub takeaways: Vec<Takeaway>,
}

/// What finalize knows about a game once it is over.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GameFacts {
    pub recording_id: Option<i64>,
    pub riot_game_id: Option<i64>,
    pub champion: Option<String>,
    pub matchup: Option<String>,
    pub result: Option<GameResult>,
    pub ended_at: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockSpan {
    pub id: i64,
    pub started_at: i64,
    pub ended_at: i64,
}

/// Which block a game played at `at` joins, or `None` for a new one.
///
/// A game joins a block when it falls within `BLOCK_GAP_MS` of it on either
/// side, so the rule works for the importer's historical games as well as
/// for a game starting now. Where two blocks qualify, the nearer wins, and a
/// tie goes to the earlier block.
pub fn choose_block(candidates: &[BlockSpan], at: i64) -> Option<i64> {
    candidates
        .iter()
        .filter_map(|b| {
            let distance = if at < b.started_at {
                b.started_at - at
            } else {
                (at - b.ended_at).max(0)
            };
            (distance <= BLOCK_GAP_MS).then_some((distance, b.started_at, b.id))
        })
        .min()
        .map(|(_, _, id)| id)
}

/// The enemy who played our position, from a scoreboard's JSON. The same
/// rule as `laneOpponent` in `src/lib/library/scoreboard.ts`: a lookup,
/// never a guess, so `None` wherever positions are missing.
pub fn lane_opponent(scoreboard: &crate::live_client::events::Scoreboard) -> Option<String> {
    let us = scoreboard.players.iter().find(|p| p.is_us)?;
    let position = us.position.as_deref()?;
    scoreboard
        .players
        .iter()
        .find(|p| p.team != us.team && p.position.as_deref() == Some(position))
        .map(|p| p.champion.clone())
}

fn lane_opponent_json(scoreboard_json: Option<&str>) -> Option<String> {
    serde_json::from_str(scoreboard_json?).ok().as_ref().and_then(lane_opponent)
}

fn refuse(message: impl Into<String>) -> DbError {
    DbError::Refused(message.into())
}

fn non_empty(body: &str, what: &str) -> Result<String, DbError> {
    let body = body.trim();
    if body.is_empty() {
        return Err(refuse(format!("{what} cannot be empty")));
    }
    Ok(body.to_string())
}

fn row_to_objective(row: &rusqlite::Row) -> rusqlite::Result<Objective> {
    Ok(Objective {
        id: row.get(0)?,
        body: row.get(1)?,
        category: row.get(2)?,
        status: row.get(3)?,
        created_at: row.get(4)?,
        retired_at: row.get(5)?,
    })
}

fn row_to_takeaway(row: &rusqlite::Row) -> rusqlite::Result<Takeaway> {
    Ok(Takeaway {
        id: row.get(0)?,
        game_id: row.get(1)?,
        block_id: row.get(2)?,
        body: row.get(3)?,
        objective_id: row.get(4)?,
        promoted_to_id: row.get(5)?,
        created_at: row.get(6)?,
    })
}

const OBJECTIVE_COLUMNS: &str = "id, body, category, status, created_at, retired_at";
const TAKEAWAY_COLUMNS: &str =
    "id, game_id, block_id, body, objective_id, promoted_to_id, created_at";

fn get_objective(tx: &rusqlite::Connection, id: i64) -> Result<Objective, DbError> {
    tx.query_row(
        &format!("SELECT {OBJECTIVE_COLUMNS} FROM objectives WHERE id = ?1"),
        [id],
        row_to_objective,
    )
    .optional()?
    .ok_or_else(|| refuse(format!("no objective {id}")))
}

fn get_takeaway(tx: &rusqlite::Connection, id: i64) -> Result<Takeaway, DbError> {
    tx.query_row(
        &format!("SELECT {TAKEAWAY_COLUMNS} FROM takeaways WHERE id = ?1"),
        [id],
        row_to_takeaway,
    )
    .optional()?
    .ok_or_else(|| refuse(format!("no takeaway {id}")))
}

/// Puts a game in the block `choose_block` picks, or a new one, and widens
/// that block to cover it.
pub(super) fn assign_block(tx: &Transaction, game_id: i64, started_at: i64) -> Result<i64, DbError> {
    let candidates = {
        let mut stmt = tx.prepare(
            "SELECT id, started_at, ended_at FROM blocks
             WHERE started_at - ?2 <= ?1 AND ended_at + ?2 >= ?1",
        )?;
        stmt.query_map(params![started_at, BLOCK_GAP_MS], |r| {
            Ok(BlockSpan { id: r.get(0)?, started_at: r.get(1)?, ended_at: r.get(2)? })
        })?
        .collect::<Result<Vec<_>, _>>()?
    };
    let block_id = match choose_block(&candidates, started_at) {
        Some(id) => id,
        None => {
            tx.execute(
                "INSERT INTO blocks (started_at, ended_at) VALUES (?1, ?1)",
                [started_at],
            )?;
            tx.last_insert_rowid()
        }
    };
    tx.execute("UPDATE games SET block_id = ?1 WHERE id = ?2", [block_id, game_id])?;
    refit_block(tx, block_id)?;
    Ok(block_id)
}

/// Sets a block's bounds from its games. A game still in progress counts
/// from its start. A block with no games keeps the bounds it had.
fn refit_block(tx: &Transaction, block_id: i64) -> Result<(), DbError> {
    tx.execute(
        "UPDATE blocks SET
            started_at = COALESCE((SELECT MIN(started_at) FROM games WHERE block_id = ?1), started_at),
            ended_at = COALESCE(
                (SELECT MAX(COALESCE(ended_at, started_at)) FROM games WHERE block_id = ?1),
                ended_at)
         WHERE id = ?1",
        [block_id],
    )?;
    Ok(())
}

/// `merge_blocks`, inside a transaction the caller owns: the importer merges
/// as part of its own.
pub(super) fn merge_blocks_in(tx: &Transaction, into: i64, from: i64) -> Result<(), DbError> {
    if into == from {
        return Err(refuse("a block cannot be merged into itself"));
    }
    for id in [into, from] {
        let exists: bool =
            tx.query_row("SELECT EXISTS (SELECT 1 FROM blocks WHERE id = ?1)", [id], |r| r.get(0))?;
        if !exists {
            return Err(refuse(format!("no block {id}")));
        }
    }
    tx.execute("UPDATE games SET block_id = ?1 WHERE block_id = ?2", [into, from])?;
    tx.execute("UPDATE takeaways SET block_id = ?1 WHERE block_id = ?2", [into, from])?;
    // The absorbed block's bounds count even if it had no games.
    tx.execute(
        "UPDATE blocks SET
            started_at = MIN(started_at, (SELECT started_at FROM blocks WHERE id = ?2)),
            ended_at = MAX(ended_at, (SELECT ended_at FROM blocks WHERE id = ?2))
         WHERE id = ?1",
        [into, from],
    )?;
    tx.execute("DELETE FROM blocks WHERE id = ?1", [from])?;
    refit_block(tx, into)?;
    Ok(())
}

/// `ensure_game_for_recording`, inside a transaction the caller owns: the
/// importer matches recordings from before WS9 as part of its own.
pub(super) fn ensure_game_in(tx: &Transaction, recording_id: i64) -> Result<i64, DbError> {
    if let Some(id) = tx
        .query_row("SELECT id FROM games WHERE recording_id = ?1", [recording_id], |r| r.get(0))
        .optional()?
    {
        return Ok(id);
    }
    let recording = tx
        .query_row(
            "SELECT started_at, duration_s, champion, win, scoreboard_json, game_id
             FROM recordings WHERE id = ?1",
            [recording_id],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, Option<f64>>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<bool>>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, Option<i64>>(5)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| refuse(format!("no recording {recording_id}")))?;
    let (started_at, duration_s, champion, win, scoreboard_json, riot_game_id) = recording;
    let ended_at = duration_s.map(|d| started_at + (d * 1000.0) as i64);
    let result = win.map(|w| if w { GameResult::Win } else { GameResult::Loss });
    tx.execute(
        "INSERT INTO games
            (recording_id, riot_game_id, started_at, ended_at, champion, matchup, result)
         VALUES (?1,
                 CASE WHEN EXISTS (SELECT 1 FROM games WHERE riot_game_id = ?2) THEN NULL ELSE ?2 END,
                 ?3, ?4, ?5, ?6, ?7)",
        params![
            recording_id,
            riot_game_id,
            started_at,
            ended_at,
            champion,
            lane_opponent_json(scoreboard_json.as_deref()),
            result,
        ],
    )?;
    let game_id = tx.last_insert_rowid();
    assign_block(tx, game_id, started_at)?;
    Ok(game_id)
}

fn snapshot_active_objectives(tx: &Transaction, game_id: i64) -> Result<(), DbError> {
    tx.execute(
        "INSERT OR IGNORE INTO game_objectives (game_id, objective_id)
         SELECT ?1, id FROM objectives WHERE status = 'active'",
        [game_id],
    )?;
    Ok(())
}

impl Db {
    // --- games ------------------------------------------------------------

    /// Opens a game as it starts: its row, its block, and the objectives it
    /// is played against, in one transaction.
    pub fn start_game(&self, recording_id: Option<i64>, started_at: i64) -> Result<i64, DbError> {
        let mut conn = self.pool.write();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO games (recording_id, started_at) VALUES (?1, ?2)",
            params![recording_id, started_at],
        )?;
        let game_id = tx.last_insert_rowid();
        assign_block(&tx, game_id, started_at)?;
        snapshot_active_objectives(&tx, game_id)?;
        tx.commit()?;
        Ok(game_id)
    }

    /// Completes a game from what finalize knows, and widens its block to
    /// its end. A fact that is `None` leaves the column as it was.
    ///
    /// `riot_game_id` is only taken if no other game holds it. Two games
    /// claiming one Riot game is a bug somewhere else, and refusing the whole
    /// finalize over it would lose the facts that are right.
    pub fn finish_game(&self, game_id: i64, facts: &GameFacts) -> Result<(), DbError> {
        let mut conn = self.pool.write();
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE games SET
                recording_id = COALESCE(recording_id, ?2),
                riot_game_id = CASE
                    WHEN ?3 IS NOT NULL
                     AND NOT EXISTS (SELECT 1 FROM games WHERE riot_game_id = ?3 AND id <> ?1)
                    THEN ?3 ELSE riot_game_id END,
                champion = COALESCE(?4, champion),
                matchup = COALESCE(?5, matchup),
                result = COALESCE(?6, result),
                ended_at = COALESCE(?7, ended_at)
             WHERE id = ?1",
            params![
                game_id,
                facts.recording_id,
                facts.riot_game_id,
                facts.champion,
                facts.matchup,
                facts.result,
                facts.ended_at,
            ],
        )?;
        let block_id: Option<i64> =
            tx.query_row("SELECT block_id FROM games WHERE id = ?1", [game_id], |r| r.get(0))?;
        if let Some(block_id) = block_id {
            refit_block(&tx, block_id)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// The game a recording belongs to, creating it from the recording if
    /// there is none. That is every recording made before WS9, anything
    /// `reconcile` imported, and a finalize whose start-insert failed.
    ///
    /// **No objectives are snapshotted** for a game made this way: which
    /// ones were active when it was played is not known, and the ones active
    /// now would be a guess dressed as history.
    pub fn ensure_game_for_recording(&self, recording_id: i64) -> Result<i64, DbError> {
        let mut conn = self.pool.write();
        let tx = conn.transaction()?;
        let game_id = ensure_game_in(&tx, recording_id)?;
        tx.commit()?;
        Ok(game_id)
    }

    /// Everything the review form shows for one game, or `None` if there is
    /// no such game.
    pub fn get_game_review(&self, game_id: i64) -> Result<Option<GameReview>, DbError> {
        let conn = self.pool.read();
        let Some((game, scoreboard_json, death_markers)) = conn
            .query_row(
                "SELECT g.id, g.recording_id, g.started_at, g.ended_at, g.block_id,
                        COALESCE(r.champion, g.champion), g.matchup,
                        COALESCE(CASE r.win WHEN 1 THEN 'win' WHEN 0 THEN 'loss' END, g.result),
                        r.scoreboard_json,
                        CASE WHEN EXISTS (SELECT 1 FROM markers WHERE recording_id = r.id)
                             THEN (SELECT COUNT(*) FROM markers
                                   WHERE recording_id = r.id AND kind = 'death')
                        END
                 FROM games g LEFT JOIN recordings r ON r.id = g.recording_id
                 WHERE g.id = ?1",
                [game_id],
                |r| {
                    Ok((
                        GameSummary {
                            id: r.get(0)?,
                            recording_id: r.get(1)?,
                            started_at: r.get(2)?,
                            ended_at: r.get(3)?,
                            block_id: r.get(4)?,
                            champion: r.get(5)?,
                            matchup: r.get(6)?,
                            result: r.get(7)?,
                        },
                        r.get::<_, Option<String>>(8)?,
                        r.get::<_, Option<i64>>(9)?,
                    ))
                },
            )
            .optional()?
        else {
            return Ok(None);
        };
        let game = GameSummary {
            matchup: lane_opponent_json(scoreboard_json.as_deref()).or(game.matchup),
            ..game
        };

        let review = conn
            .query_row(
                "SELECT game_rating, lane_rating, mental_rating, first_clear_ms,
                        smites_at_clear, deaths, free_notes
                 FROM game_reviews WHERE game_id = ?1",
                [game_id],
                |r| {
                    Ok(ReviewInput {
                        game_rating: r.get(0)?,
                        lane_rating: r.get(1)?,
                        mental_rating: r.get(2)?,
                        first_clear_ms: r.get(3)?,
                        smites_at_clear: r.get(4)?,
                        deaths: r.get(5)?,
                        free_notes: r.get(6)?,
                    })
                },
            )
            .optional()?;

        let objectives = conn
            .prepare(
                "SELECT o.id, o.body, o.category, o.status, go.ticked
                 FROM game_objectives go JOIN objectives o ON o.id = go.objective_id
                 WHERE go.game_id = ?1
                 ORDER BY o.created_at, o.id",
            )?
            .query_map([game_id], |r| {
                Ok(GameObjective {
                    objective_id: r.get(0)?,
                    body: r.get(1)?,
                    category: r.get(2)?,
                    status: r.get(3)?,
                    ticked: r.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let takeaways = conn
            .prepare(&format!(
                "SELECT {TAKEAWAY_COLUMNS} FROM takeaways WHERE game_id = ?1 ORDER BY created_at, id"
            ))?
            .query_map([game_id], row_to_takeaway)?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Some(GameReview { game, review, death_markers, objectives, takeaways }))
    }

    // --- reviews ----------------------------------------------------------

    pub fn upsert_review(&self, game_id: i64, review: &ReviewInput) -> Result<(), DbError> {
        for (value, what) in [
            (review.first_clear_ms, "clear time"),
            (review.smites_at_clear, "smites"),
            (review.deaths, "deaths"),
        ] {
            if value.is_some_and(|v| v < 0) {
                return Err(refuse(format!("{what} cannot be negative")));
            }
        }
        let conn = self.pool.write();
        let changed = conn.execute(
            "INSERT INTO game_reviews
                (game_id, game_rating, lane_rating, mental_rating,
                 first_clear_ms, smites_at_clear, deaths, free_notes)
             SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8 WHERE EXISTS (SELECT 1 FROM games WHERE id = ?1)
             ON CONFLICT (game_id) DO UPDATE SET
                game_rating = excluded.game_rating,
                lane_rating = excluded.lane_rating,
                mental_rating = excluded.mental_rating,
                first_clear_ms = excluded.first_clear_ms,
                smites_at_clear = excluded.smites_at_clear,
                deaths = excluded.deaths,
                free_notes = excluded.free_notes",
            params![
                game_id,
                review.game_rating,
                review.lane_rating,
                review.mental_rating,
                review.first_clear_ms,
                review.smites_at_clear,
                review.deaths,
                review.free_notes,
            ],
        )?;
        if changed == 0 {
            return Err(refuse(format!("no game {game_id}")));
        }
        Ok(())
    }

    // --- objectives -------------------------------------------------------

    pub fn create_objective(
        &self,
        body: &str,
        category: ObjectiveCategory,
        now: i64,
    ) -> Result<Objective, DbError> {
        let body = non_empty(body, "an objective")?;
        let conn = self.pool.write();
        conn.execute(
            "INSERT INTO objectives (body, category, created_at) VALUES (?1, ?2, ?3)",
            params![body, category, now],
        )?;
        get_objective(&conn, conn.last_insert_rowid())
    }

    pub fn update_objective(
        &self,
        id: i64,
        body: &str,
        category: ObjectiveCategory,
    ) -> Result<Objective, DbError> {
        let body = non_empty(body, "an objective")?;
        let conn = self.pool.write();
        conn.execute(
            "UPDATE objectives SET body = ?2, category = ?3 WHERE id = ?1",
            params![id, body, category],
        )?;
        get_objective(&conn, id)
    }

    /// `retired_at` is stamped on retiring and cleared on anything else, so
    /// an objective brought back is not still dated as retired.
    pub fn set_objective_status(
        &self,
        id: i64,
        status: ObjectiveStatus,
        now: i64,
    ) -> Result<Objective, DbError> {
        let conn = self.pool.write();
        conn.execute(
            "UPDATE objectives SET
                status = ?2,
                retired_at = CASE WHEN ?2 = 'retired' THEN COALESCE(retired_at, ?3) END
             WHERE id = ?1",
            params![id, status, now],
        )?;
        get_objective(&conn, id)
    }

    /// Newest first. `None` lists every status.
    pub fn list_objectives(&self, status: Option<ObjectiveStatus>) -> Result<Vec<Objective>, DbError> {
        let conn = self.pool.read();
        let mut stmt = conn.prepare(&format!(
            "SELECT {OBJECTIVE_COLUMNS} FROM objectives
             WHERE ?1 IS NULL OR status = ?1
             ORDER BY created_at DESC, id DESC"
        ))?;
        let rows = stmt.query_map([status], row_to_objective)?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn set_objective_ticked(
        &self,
        game_id: i64,
        objective_id: i64,
        ticked: bool,
    ) -> Result<(), DbError> {
        let conn = self.pool.write();
        let changed = conn.execute(
            "UPDATE game_objectives SET ticked = ?3 WHERE game_id = ?1 AND objective_id = ?2",
            params![game_id, objective_id, ticked],
        )?;
        if changed == 0 {
            return Err(refuse(format!(
                "objective {objective_id} was not active when game {game_id} started"
            )));
        }
        Ok(())
    }

    // --- blocks -----------------------------------------------------------

    /// Starts a new block at `game_id`: it and every later game in its block
    /// move to the new one. Block takeaways stay where they were. Returns the
    /// new block.
    pub fn split_block(&self, game_id: i64) -> Result<i64, DbError> {
        let mut conn = self.pool.write();
        let tx = conn.transaction()?;
        let (block_id, started_at): (Option<i64>, i64) = tx
            .query_row("SELECT block_id, started_at FROM games WHERE id = ?1", [game_id], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()?
            .ok_or_else(|| refuse(format!("no game {game_id}")))?;
        let block_id = block_id.ok_or_else(|| refuse(format!("game {game_id} is in no block")))?;
        let earlier: i64 = tx.query_row(
            "SELECT COUNT(*) FROM games
             WHERE block_id = ?1 AND (started_at < ?2 OR (started_at = ?2 AND id < ?3))",
            params![block_id, started_at, game_id],
            |r| r.get(0),
        )?;
        if earlier == 0 {
            return Err(refuse("the first game of a block already starts it"));
        }
        tx.execute(
            "INSERT INTO blocks (started_at, ended_at) VALUES (?1, ?1)",
            [started_at],
        )?;
        let new_block = tx.last_insert_rowid();
        tx.execute(
            "UPDATE games SET block_id = ?1
             WHERE block_id = ?2 AND (started_at > ?3 OR (started_at = ?3 AND id >= ?4))",
            params![new_block, block_id, started_at, game_id],
        )?;
        refit_block(&tx, block_id)?;
        refit_block(&tx, new_block)?;
        tx.commit()?;
        Ok(new_block)
    }

    /// Moves everything in `from` into `into` and deletes `from`.
    pub fn merge_blocks(&self, into: i64, from: i64) -> Result<(), DbError> {
        let mut conn = self.pool.write();
        let tx = conn.transaction()?;
        merge_blocks_in(&tx, into, from)?;
        tx.commit()?;
        Ok(())
    }

    // --- takeaways --------------------------------------------------------

    pub fn add_takeaway(
        &self,
        owner: TakeawayOwner,
        body: &str,
        now: i64,
    ) -> Result<Takeaway, DbError> {
        let body = non_empty(body, "a takeaway")?;
        let (game_id, block_id) = match owner {
            TakeawayOwner::Game(id) => (Some(id), None),
            TakeawayOwner::Block(id) => (None, Some(id)),
        };
        let conn = self.pool.write();
        conn.execute(
            "INSERT INTO takeaways (game_id, block_id, body, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![game_id, block_id, body, now],
        )
        .map_err(|e| match e {
            rusqlite::Error::SqliteFailure(f, _)
                if f.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                refuse(format!("no such {owner:?}"))
            }
            e => e.into(),
        })?;
        get_takeaway(&conn, conn.last_insert_rowid())
    }

    pub fn delete_takeaway(&self, id: i64) -> Result<(), DbError> {
        self.pool.write().execute("DELETE FROM takeaways WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Turns a takeaway into an active objective, in one transaction: the
    /// objective is created from its body and the takeaway points at it.
    /// A takeaway already promoted returns the objective it became, so a
    /// double click does not make two.
    pub fn promote_takeaway(
        &self,
        id: i64,
        category: ObjectiveCategory,
        now: i64,
    ) -> Result<Objective, DbError> {
        let mut conn = self.pool.write();
        let tx = conn.transaction()?;
        let takeaway = get_takeaway(&tx, id)?;
        if let Some(existing) = takeaway.promoted_to_id {
            return get_objective(&tx, existing);
        }
        tx.execute(
            "INSERT INTO objectives (body, category, created_at) VALUES (?1, ?2, ?3)",
            params![takeaway.body, category, now],
        )?;
        let objective_id = tx.last_insert_rowid();
        tx.execute(
            "UPDATE takeaways SET promoted_to_id = ?1 WHERE id = ?2",
            [objective_id, id],
        )?;
        let objective = get_objective(&tx, objective_id)?;
        tx.commit()?;
        Ok(objective)
    }
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use super::*;

    fn db() -> Db {
        Db::open_temporary().unwrap()
    }

    fn exec(db: &Db, sql: &str) -> rusqlite::Result<usize> {
        db.pool.write().execute(sql, [])
    }

    fn count(db: &Db, sql: &str) -> i64 {
        db.pool.write().query_row(sql, [], |r| r.get(0)).unwrap()
    }

    fn is_constraint_error(result: rusqlite::Result<usize>) -> bool {
        matches!(
            result,
            Err(rusqlite::Error::SqliteFailure(e, _))
                if e.code == rusqlite::ErrorCode::ConstraintViolation
        )
    }

    /// One game with a review, an objective snapshot, a note and a takeaway.
    fn seed_game(db: &Db) {
        exec(db, "INSERT INTO games (id, started_at) VALUES (1, 1000)").unwrap();
        exec(db, "INSERT INTO game_reviews (game_id, game_rating) VALUES (1, 'win')").unwrap();
        exec(db, "INSERT INTO objectives (id, body, created_at) VALUES (1, 'track the jungler', 1000)")
            .unwrap();
        exec(db, "INSERT INTO game_objectives (game_id, objective_id) VALUES (1, 1)").unwrap();
        exec(db, "INSERT INTO notes (game_id, ts_ms, kind, body) VALUES (1, 60000, 'good', 'n')")
            .unwrap();
        exec(db, "INSERT INTO takeaways (game_id, body, created_at) VALUES (1, 't', 1000)").unwrap();
    }

    #[test]
    fn every_enum_refuses_a_value_outside_its_set() {
        let db = db();
        seed_game(&db);
        for sql in [
            "UPDATE games SET result = 'draw'",
            "UPDATE game_reviews SET game_rating = 'neutral'",
            "UPDATE game_reviews SET lane_rating = 'good'",
            "UPDATE game_reviews SET mental_rating = 'win'",
            "UPDATE notes SET kind = 'comment'",
            "UPDATE objectives SET status = 'done'",
            "UPDATE objectives SET category = 'vision'",
            "UPDATE game_objectives SET ticked = 2",
        ] {
            assert!(is_constraint_error(exec(&db, sql)), "{sql} should be refused");
        }
    }

    #[test]
    fn every_enum_accepts_each_value_in_its_set() {
        let db = db();
        seed_game(&db);
        let sets: [(&str, &[&str]); 7] = [
            ("UPDATE games SET result = '{}'", &["win", "loss"]),
            ("UPDATE game_reviews SET game_rating = '{}'", &["win", "loss"]),
            ("UPDATE game_reviews SET lane_rating = '{}'", &["win", "neutral", "loss"]),
            ("UPDATE game_reviews SET mental_rating = '{}'", &["good", "neutral", "bad"]),
            ("UPDATE notes SET kind = '{}'", &["mistake", "good", "question", "takeaway"]),
            ("UPDATE objectives SET status = '{}'", &["active", "paused", "retired"]),
            (
                "UPDATE objectives SET category = '{}'",
                &["macro", "lane", "mental", "mechanics", "other"],
            ),
        ];
        for (template, values) in sets {
            for value in values {
                let sql = template.replace("{}", value);
                exec(&db, &sql).unwrap_or_else(|e| panic!("{sql}: {e}"));
            }
        }
        // Unset is a legal state for every rating.
        exec(&db, "UPDATE game_reviews SET game_rating = NULL, lane_rating = NULL, mental_rating = NULL")
            .unwrap();
    }

    #[test]
    fn a_takeaway_belongs_to_exactly_one_of_a_game_or_a_block() {
        let db = db();
        seed_game(&db);
        exec(&db, "INSERT INTO blocks (id, started_at, ended_at) VALUES (1, 0, 0)").unwrap();

        exec(&db, "INSERT INTO takeaways (block_id, body, created_at) VALUES (1, 'b', 0)").unwrap();
        assert!(is_constraint_error(exec(
            &db,
            "INSERT INTO takeaways (game_id, block_id, body, created_at) VALUES (1, 1, 'both', 0)"
        )));
        assert!(is_constraint_error(exec(
            &db,
            "INSERT INTO takeaways (body, created_at) VALUES ('neither', 0)"
        )));
    }

    #[test]
    fn a_game_has_at_most_one_review() {
        let db = db();
        seed_game(&db);
        assert!(is_constraint_error(exec(&db, "INSERT INTO game_reviews (game_id) VALUES (1)")));
        assert!(is_constraint_error(exec(&db, "INSERT INTO game_reviews (game_id) VALUES (99)")));
    }

    #[test]
    fn an_objective_is_snapshotted_into_a_game_once() {
        let db = db();
        seed_game(&db);
        assert!(is_constraint_error(exec(
            &db,
            "INSERT INTO game_objectives (game_id, objective_id) VALUES (1, 1)"
        )));
    }

    /// The reason `games` exists: deleting a VOD, by any of the three paths
    /// that do it, must not take the review with it.
    #[test]
    fn deleting_a_recording_keeps_its_game_and_review() {
        let db = db();
        let recording = db.begin_recording("C:/vods/a.mp4", 1000).unwrap();
        db.insert_markers(recording, &[NewMarker {
            game_time_s: 60.0,
            video_time_s: 65.0,
            kind: "death".into(),
            payload_json: "{}".into(),
        }])
        .unwrap();
        seed_game(&db);
        exec(&db, &format!("UPDATE games SET recording_id = {recording}")).unwrap();
        let marker = count(&db, "SELECT id FROM markers");
        exec(&db, &format!("UPDATE notes SET marker_id = {marker}")).unwrap();

        db.delete_recording(recording).unwrap();

        assert_eq!(count(&db, "SELECT COUNT(*) FROM games WHERE recording_id IS NULL"), 1);
        assert_eq!(count(&db, "SELECT COUNT(*) FROM game_reviews"), 1);
        assert_eq!(count(&db, "SELECT COUNT(*) FROM takeaways"), 1);
        assert_eq!(count(&db, "SELECT COUNT(*) FROM game_objectives"), 1);
        assert_eq!(
            count(&db, "SELECT COUNT(*) FROM notes WHERE marker_id IS NULL"),
            1,
            "the note stays and loses its link to the deleted marker"
        );
    }

    #[test]
    fn deleting_a_game_takes_everything_hung_off_it() {
        let db = db();
        seed_game(&db);
        exec(&db, "DELETE FROM games").unwrap();
        for table in ["game_reviews", "game_objectives", "notes", "takeaways"] {
            assert_eq!(count(&db, &format!("SELECT COUNT(*) FROM {table}")), 0, "{table}");
        }
        assert_eq!(count(&db, "SELECT COUNT(*) FROM objectives"), 1, "objectives outlive games");
    }

    #[test]
    fn deleting_an_objective_unlinks_notes_and_takeaways_rather_than_deleting_them() {
        let db = db();
        seed_game(&db);
        exec(&db, "UPDATE notes SET objective_id = 1").unwrap();
        exec(&db, "UPDATE takeaways SET objective_id = 1, promoted_to_id = 1").unwrap();
        exec(&db, "DELETE FROM objectives").unwrap();
        assert_eq!(count(&db, "SELECT COUNT(*) FROM notes WHERE objective_id IS NULL"), 1);
        assert_eq!(
            count(&db, "SELECT COUNT(*) FROM takeaways WHERE objective_id IS NULL AND promoted_to_id IS NULL"),
            1
        );
        assert_eq!(count(&db, "SELECT COUNT(*) FROM game_objectives"), 0);
    }

    #[test]
    fn deleting_a_block_unlinks_its_games() {
        let db = db();
        seed_game(&db);
        exec(&db, "INSERT INTO blocks (id, started_at, ended_at) VALUES (1, 0, 0)").unwrap();
        exec(&db, "UPDATE games SET block_id = 1").unwrap();
        exec(&db, "DELETE FROM blocks").unwrap();
        assert_eq!(count(&db, "SELECT COUNT(*) FROM games WHERE block_id IS NULL"), 1);
    }

    #[test]
    fn a_recording_and_a_riot_game_each_have_at_most_one_game() {
        let db = db();
        let recording = db.begin_recording("C:/vods/a.mp4", 1000).unwrap();
        exec(&db, &format!("INSERT INTO games (recording_id, riot_game_id, started_at) VALUES ({recording}, 7, 0)"))
            .unwrap();
        assert!(is_constraint_error(exec(
            &db,
            &format!("INSERT INTO games (recording_id, started_at) VALUES ({recording}, 0)")
        )));
        assert!(is_constraint_error(exec(&db, "INSERT INTO games (riot_game_id, started_at) VALUES (7, 0)")));
        // Unrecorded, unidentified games are the importer's normal case.
        exec(&db, "INSERT INTO games (started_at) VALUES (0)").unwrap();
        exec(&db, "INSERT INTO games (started_at) VALUES (0)").unwrap();
    }

    #[test]
    fn the_indexes_the_review_queries_rely_on_exist() {
        let db = db();
        for index in [
            "idx_games_started_at",
            "idx_games_block_id",
            "idx_objectives_status",
            "idx_notes_game_id_ts_ms",
            "idx_takeaways_game_id",
            "idx_takeaways_block_id",
        ] {
            assert_eq!(
                count(&db, &format!("SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = '{index}'")),
                1,
                "{index}"
            );
        }
    }

    /// A library from the build before WS9 keeps its recordings and gains
    /// empty review tables.
    #[test]
    fn a_library_from_before_ws9_migrates_forward() {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        let (migrations, latest) = &*MIGRATIONS;
        migrations.to_version(&mut conn, (*latest - 1) as usize).unwrap();
        conn.execute("INSERT INTO recordings (path, started_at, finished_at) VALUES ('a.mp4', 1, 1)", [])
            .unwrap();

        Db::init(&mut conn).unwrap();

        let recordings: i64 = conn.query_row("SELECT COUNT(*) FROM recordings", [], |r| r.get(0)).unwrap();
        let games: i64 = conn.query_row("SELECT COUNT(*) FROM games", [], |r| r.get(0)).unwrap();
        assert_eq!((recordings, games), (1, 0));
    }

    // --- the block rule, pure ------------------------------------------------

    const HOUR: i64 = 60 * 60 * 1000;

    fn span(id: i64, started_at: i64, ended_at: i64) -> BlockSpan {
        BlockSpan { id, started_at, ended_at }
    }

    #[test]
    fn a_game_within_two_hours_of_the_last_one_ending_joins_its_block() {
        let blocks = [span(1, 0, 3 * HOUR)];
        assert_eq!(choose_block(&blocks, 3 * HOUR + 2 * HOUR - 60_000), Some(1), "1h59 later");
        assert_eq!(choose_block(&blocks, 3 * HOUR + 2 * HOUR), Some(1), "exactly 2h later");
        assert_eq!(choose_block(&blocks, 3 * HOUR + 2 * HOUR + 60_000), None, "2h01 later");
    }

    #[test]
    fn a_game_inside_a_block_joins_it() {
        assert_eq!(choose_block(&[span(1, 0, 3 * HOUR)], HOUR), Some(1));
    }

    /// The importer adds history in any order, so a game can land just
    /// before a block as well as just after one.
    #[test]
    fn a_game_shortly_before_a_block_joins_it() {
        let blocks = [span(1, 10 * HOUR, 12 * HOUR)];
        assert_eq!(choose_block(&blocks, 9 * HOUR), Some(1));
        assert_eq!(choose_block(&blocks, 7 * HOUR), None);
    }

    #[test]
    fn where_two_blocks_qualify_the_nearer_wins_and_a_tie_goes_to_the_earlier() {
        let blocks = [span(2, 5 * HOUR, 6 * HOUR), span(1, 0, HOUR)];
        assert_eq!(choose_block(&blocks, 2 * HOUR), Some(1));
        assert_eq!(choose_block(&blocks, 4 * HOUR), Some(2));
        assert_eq!(choose_block(&blocks, 3 * HOUR), Some(1), "equidistant");
    }

    #[test]
    fn no_blocks_means_a_new_one() {
        assert_eq!(choose_block(&[], 0), None);
    }

    #[test]
    fn every_enum_round_trips_through_sqlite() {
        let db = db();
        seed_game(&db);
        let conn = db.pool.write();
        for v in GameResult::ALL {
            conn.execute("UPDATE game_reviews SET game_rating = ?1", [v]).unwrap();
            let back: GameResult =
                conn.query_row("SELECT game_rating FROM game_reviews", [], |r| r.get(0)).unwrap();
            assert_eq!(back, *v);
        }
        for v in LaneRating::ALL {
            conn.execute("UPDATE game_reviews SET lane_rating = ?1", [v]).unwrap();
        }
        for v in MentalRating::ALL {
            conn.execute("UPDATE game_reviews SET mental_rating = ?1", [v]).unwrap();
        }
        for v in ObjectiveStatus::ALL {
            conn.execute("UPDATE objectives SET status = ?1", [v]).unwrap();
        }
        for v in ObjectiveCategory::ALL {
            conn.execute("UPDATE objectives SET category = ?1", [v]).unwrap();
        }
    }

    // --- games and blocks, through the database ------------------------------

    fn block_of(db: &Db, game_id: i64) -> Option<i64> {
        db.pool.read().query_row("SELECT block_id FROM games WHERE id = ?1", [game_id], |r| r.get(0)).unwrap()
    }

    fn bounds(db: &Db, block_id: i64) -> (i64, i64) {
        db.pool
            .read()
            .query_row("SELECT started_at, ended_at FROM blocks WHERE id = ?1", [block_id], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap()
    }

    fn finish(db: &Db, game_id: i64, ended_at: i64) {
        db.finish_game(game_id, &GameFacts { ended_at: Some(ended_at), ..Default::default() }).unwrap();
    }

    #[test]
    fn games_an_evening_apart_share_a_block_and_the_next_day_starts_another() {
        let db = db();
        let a = db.start_game(None, 0).unwrap();
        finish(&db, a, HOUR / 2);
        let b = db.start_game(None, HOUR / 2 + 2 * HOUR - 60_000).unwrap();
        finish(&db, b, 3 * HOUR);
        let c = db.start_game(None, 24 * HOUR).unwrap();

        assert_eq!(block_of(&db, a), block_of(&db, b));
        assert_ne!(block_of(&db, b), block_of(&db, c));
        assert_eq!(bounds(&db, block_of(&db, a).unwrap()), (0, 3 * HOUR));
    }

    #[test]
    fn a_game_starting_2h01_after_the_last_ended_opens_a_new_block() {
        let db = db();
        let a = db.start_game(None, 0).unwrap();
        finish(&db, a, HOUR);
        let b = db.start_game(None, HOUR + 2 * HOUR + 60_000).unwrap();
        assert_ne!(block_of(&db, a), block_of(&db, b));
    }

    #[test]
    fn a_game_snapshots_the_objectives_active_when_it_starts() {
        let db = db();
        let active = db.create_objective("ward at 2:45", ObjectiveCategory::Macro, 0).unwrap();
        let paused = db.create_objective("track jungle", ObjectiveCategory::Lane, 0).unwrap();
        db.set_objective_status(paused.id, ObjectiveStatus::Paused, 0).unwrap();

        let game = db.start_game(None, 1000).unwrap();
        // Retiring it afterwards does not rewrite what the game was played against.
        db.set_objective_status(active.id, ObjectiveStatus::Retired, 2000).unwrap();
        let later = db.create_objective("new one", ObjectiveCategory::Other, 3000).unwrap();

        let review = db.get_game_review(game).unwrap().unwrap();
        let ids: Vec<i64> = review.objectives.iter().map(|o| o.objective_id).collect();
        assert_eq!(ids, vec![active.id]);
        assert_eq!(review.objectives[0].status, ObjectiveStatus::Retired);
        assert!(!ids.contains(&later.id));
    }

    #[test]
    fn ticking_an_objective_the_game_was_not_played_against_is_refused() {
        let db = db();
        let o = db.create_objective("x", ObjectiveCategory::Other, 0).unwrap();
        let game = db.start_game(None, 0).unwrap();
        db.set_objective_ticked(game, o.id, true).unwrap();
        assert!(db.get_game_review(game).unwrap().unwrap().objectives[0].ticked);
        db.set_objective_ticked(game, o.id, false).unwrap();
        assert!(!db.get_game_review(game).unwrap().unwrap().objectives[0].ticked);

        let other = db.create_objective("y", ObjectiveCategory::Other, 1).unwrap();
        assert!(matches!(db.set_objective_ticked(game, other.id, true), Err(DbError::Refused(_))));
    }

    /// The form's header prefers the recording while it exists, because the
    /// LCU corrects it after finalize, and falls back to the game's own copy
    /// once the VOD is gone.
    #[test]
    fn the_header_reads_through_to_the_recording_and_survives_its_deletion() {
        let db = db();
        let recording = db.begin_recording("C:/vods/a.mp4", 1000).unwrap();
        let game = db.start_game(Some(recording), 1000).unwrap();
        db.finish_game(game, &GameFacts {
            champion: Some("Lee Sin".into()),
            result: Some(GameResult::Loss),
            matchup: Some("Vi".into()),
            ended_at: Some(5000),
            ..Default::default()
        })
        .unwrap();
        db.pool
            .write()
            .execute("UPDATE recordings SET champion = 'Viego', win = 1 WHERE id = ?1", [recording])
            .unwrap();

        let header = db.get_game_review(game).unwrap().unwrap().game;
        assert_eq!(header.champion.as_deref(), Some("Viego"));
        assert_eq!(header.result, Some(GameResult::Win));
        assert_eq!(header.matchup.as_deref(), Some("Vi"), "no scoreboard, so the game's copy");

        db.delete_recording(recording).unwrap();
        let header = db.get_game_review(game).unwrap().unwrap().game;
        assert_eq!(header.recording_id, None);
        assert_eq!(header.champion.as_deref(), Some("Lee Sin"));
        assert_eq!(header.result, Some(GameResult::Loss));
    }

    #[test]
    fn deaths_prefill_counts_death_markers_and_is_unknown_without_any_markers() {
        let db = db();
        let recording = db.begin_recording("C:/vods/a.mp4", 0).unwrap();
        let game = db.start_game(Some(recording), 0).unwrap();
        assert_eq!(db.get_game_review(game).unwrap().unwrap().death_markers, None);

        let marker = |kind: &str| NewMarker {
            game_time_s: 1.0,
            video_time_s: 1.0,
            kind: kind.into(),
            payload_json: "{}".into(),
        };
        db.insert_markers(recording, &[marker("kill")]).unwrap();
        assert_eq!(db.get_game_review(game).unwrap().unwrap().death_markers, Some(0));
        db.insert_markers(recording, &[marker("death"), marker("death")]).unwrap();
        assert_eq!(db.get_game_review(game).unwrap().unwrap().death_markers, Some(2));

        let unrecorded = db.start_game(None, 10).unwrap();
        assert_eq!(db.get_game_review(unrecorded).unwrap().unwrap().death_markers, None);
    }

    #[test]
    fn a_review_is_saved_whole_and_overwritten_whole() {
        let db = db();
        let game = db.start_game(None, 0).unwrap();
        assert_eq!(db.get_game_review(game).unwrap().unwrap().review, None);

        let first = ReviewInput {
            game_rating: Some(GameResult::Win),
            lane_rating: Some(LaneRating::Neutral),
            mental_rating: Some(MentalRating::Good),
            first_clear_ms: Some(178_000),
            smites_at_clear: Some(1),
            deaths: Some(3),
            free_notes: "fine".into(),
        };
        db.upsert_review(game, &first).unwrap();
        assert_eq!(db.get_game_review(game).unwrap().unwrap().review, Some(first));

        let cleared = ReviewInput { free_notes: "".into(), ..Default::default() };
        db.upsert_review(game, &cleared).unwrap();
        assert_eq!(db.get_game_review(game).unwrap().unwrap().review, Some(cleared));
    }

    #[test]
    fn a_review_with_a_negative_count_or_for_no_game_is_refused() {
        let db = db();
        let game = db.start_game(None, 0).unwrap();
        let bad = ReviewInput { deaths: Some(-1), ..Default::default() };
        assert!(matches!(db.upsert_review(game, &bad), Err(DbError::Refused(_))));
        assert!(matches!(db.upsert_review(999, &ReviewInput::default()), Err(DbError::Refused(_))));
    }

    #[test]
    fn objectives_are_listed_by_status_newest_first() {
        let db = db();
        let a = db.create_objective("a", ObjectiveCategory::Macro, 1).unwrap();
        let b = db.create_objective("  b  ", ObjectiveCategory::Lane, 2).unwrap();
        assert_eq!(b.body, "b", "trimmed");
        db.set_objective_status(a.id, ObjectiveStatus::Retired, 10).unwrap();

        let active: Vec<i64> =
            db.list_objectives(Some(ObjectiveStatus::Active)).unwrap().iter().map(|o| o.id).collect();
        assert_eq!(active, vec![b.id]);
        let all: Vec<i64> = db.list_objectives(None).unwrap().iter().map(|o| o.id).collect();
        assert_eq!(all, vec![b.id, a.id]);
    }

    #[test]
    fn retiring_stamps_retired_at_and_reactivating_clears_it() {
        let db = db();
        let o = db.create_objective("a", ObjectiveCategory::Other, 1).unwrap();
        let retired = db.set_objective_status(o.id, ObjectiveStatus::Retired, 50).unwrap();
        assert_eq!(retired.retired_at, Some(50));
        let again = db.set_objective_status(o.id, ObjectiveStatus::Retired, 99).unwrap();
        assert_eq!(again.retired_at, Some(50), "retiring twice keeps the first date");
        let back = db.set_objective_status(o.id, ObjectiveStatus::Active, 100).unwrap();
        assert_eq!(back.retired_at, None);
    }

    #[test]
    fn an_objective_can_be_edited_but_not_emptied() {
        let db = db();
        let o = db.create_objective("a", ObjectiveCategory::Other, 1).unwrap();
        let edited = db.update_objective(o.id, "b", ObjectiveCategory::Mental).unwrap();
        assert_eq!((edited.body.as_str(), edited.category), ("b", ObjectiveCategory::Mental));
        assert!(matches!(db.update_objective(o.id, "  ", ObjectiveCategory::Mental), Err(DbError::Refused(_))));
        assert!(matches!(db.create_objective("", ObjectiveCategory::Other, 1), Err(DbError::Refused(_))));
    }

    #[test]
    fn takeaways_belong_to_a_game_or_a_block_and_can_be_deleted() {
        let db = db();
        let game = db.start_game(None, 0).unwrap();
        let block = block_of(&db, game).unwrap();
        let t = db.add_takeaway(TakeawayOwner::Game(game), "contest grubs with prio", 5).unwrap();
        let b = db.add_takeaway(TakeawayOwner::Block(block), "tilted after game 3", 6).unwrap();
        assert_eq!((t.game_id, t.block_id), (Some(game), None));
        assert_eq!((b.game_id, b.block_id), (None, Some(block)));

        let listed = db.get_game_review(game).unwrap().unwrap().takeaways;
        assert_eq!(listed, vec![t.clone()], "the form lists the game's own");

        db.delete_takeaway(t.id).unwrap();
        assert!(db.get_game_review(game).unwrap().unwrap().takeaways.is_empty());
        assert!(matches!(db.add_takeaway(TakeawayOwner::Game(999), "x", 0), Err(DbError::Refused(_))));
    }

    #[test]
    fn promoting_a_takeaway_makes_an_active_objective_once() {
        let db = db();
        let game = db.start_game(None, 0).unwrap();
        let t = db.add_takeaway(TakeawayOwner::Game(game), "ward river at 2:45", 5).unwrap();

        let o = db.promote_takeaway(t.id, ObjectiveCategory::Macro, 10).unwrap();
        assert_eq!((o.body.as_str(), o.status, o.category), ("ward river at 2:45", ObjectiveStatus::Active, ObjectiveCategory::Macro));
        let promoted = db.get_game_review(game).unwrap().unwrap().takeaways;
        assert_eq!(promoted[0].promoted_to_id, Some(o.id));

        let again = db.promote_takeaway(t.id, ObjectiveCategory::Macro, 11).unwrap();
        assert_eq!(again.id, o.id);
        assert_eq!(db.list_objectives(None).unwrap().len(), 1);
    }

    /// A promotion is one transaction: an objective with no takeaway
    /// pointing at it is exactly the half-state it exists to prevent.
    #[test]
    fn a_promotion_that_fails_leaves_no_objective_behind() {
        let db = db();
        let game = db.start_game(None, 0).unwrap();
        let t = db.add_takeaway(TakeawayOwner::Game(game), "x", 5).unwrap();
        db.pool
            .write()
            .execute_batch(
                "CREATE TRIGGER no_promotions BEFORE UPDATE OF promoted_to_id ON takeaways
                 BEGIN SELECT RAISE(ABORT, 'refused'); END;",
            )
            .unwrap();

        assert!(db.promote_takeaway(t.id, ObjectiveCategory::Other, 10).is_err());
        assert!(db.list_objectives(None).unwrap().is_empty());
    }

    #[test]
    fn splitting_a_block_moves_that_game_and_every_later_one() {
        let db = db();
        let games: Vec<i64> = (0..4)
            .map(|i| {
                let g = db.start_game(None, i * HOUR).unwrap();
                finish(&db, g, i * HOUR + HOUR / 2);
                g
            })
            .collect();
        let block = block_of(&db, games[0]).unwrap();
        let t = db.add_takeaway(TakeawayOwner::Block(block), "block note", 0).unwrap();

        let new_block = db.split_block(games[2]).unwrap();

        assert_eq!(block_of(&db, games[1]), Some(block));
        assert_eq!(block_of(&db, games[2]), Some(new_block));
        assert_eq!(block_of(&db, games[3]), Some(new_block));
        assert_eq!(bounds(&db, block), (0, HOUR + HOUR / 2));
        assert_eq!(bounds(&db, new_block), (2 * HOUR, 3 * HOUR + HOUR / 2));
        let kept: Option<i64> = db
            .pool
            .read()
            .query_row("SELECT block_id FROM takeaways WHERE id = ?1", [t.id], |r| r.get(0))
            .unwrap();
        assert_eq!(kept, Some(block), "block takeaways stay put");

        assert!(matches!(db.split_block(games[0]), Err(DbError::Refused(_))), "already first");
    }

    #[test]
    fn merging_blocks_moves_games_and_takeaways_and_deletes_the_emptied_one() {
        let db = db();
        let a = db.start_game(None, 0).unwrap();
        finish(&db, a, HOUR);
        let b = db.start_game(None, 10 * HOUR).unwrap();
        finish(&db, b, 11 * HOUR);
        let (first, second) = (block_of(&db, a).unwrap(), block_of(&db, b).unwrap());
        assert_ne!(first, second);
        db.add_takeaway(TakeawayOwner::Block(second), "late session", 0).unwrap();

        db.merge_blocks(first, second).unwrap();

        assert_eq!(block_of(&db, b), Some(first));
        assert_eq!(bounds(&db, first), (0, 11 * HOUR));
        assert_eq!(count(&db, "SELECT COUNT(*) FROM blocks"), 1);
        assert_eq!(count(&db, &format!("SELECT COUNT(*) FROM takeaways WHERE block_id = {first}")), 1);
        assert!(matches!(db.merge_blocks(first, first), Err(DbError::Refused(_))));
        assert!(matches!(db.merge_blocks(first, 999), Err(DbError::Refused(_))));
    }

    #[test]
    fn a_game_is_made_for_an_old_recording_once_with_no_objectives() {
        let db = db();
        db.create_objective("x", ObjectiveCategory::Other, 0).unwrap();
        let recording = db
            .insert_recording(&NewRecording {
                path: "C:/vods/old.mp4".into(),
                started_at: 1000,
                duration_s: Some(1800.0),
                champion: Some("Ahri".into()),
                win: Some(false),
                game_id: Some(42),
                finished_at: Some(1000),
                ..Default::default()
            })
            .unwrap();

        let game = db.ensure_game_for_recording(recording).unwrap();
        assert_eq!(db.ensure_game_for_recording(recording).unwrap(), game, "found, not made again");

        let review = db.get_game_review(game).unwrap().unwrap();
        assert_eq!(review.game.ended_at, Some(1000 + 1_800_000));
        assert_eq!(review.game.result, Some(GameResult::Loss));
        assert!(review.objectives.is_empty(), "no guessing at what was active then");
        assert!(review.game.block_id.is_some());
        assert!(matches!(db.ensure_game_for_recording(999), Err(DbError::Refused(_))));
    }

    #[test]
    fn a_riot_game_id_already_held_by_another_game_is_not_taken() {
        let db = db();
        let a = db.start_game(None, 0).unwrap();
        let b = db.start_game(None, 1).unwrap();
        db.finish_game(a, &GameFacts { riot_game_id: Some(7), ..Default::default() }).unwrap();
        db.finish_game(b, &GameFacts {
            riot_game_id: Some(7),
            champion: Some("Ahri".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(count(&db, "SELECT COUNT(*) FROM games WHERE riot_game_id = 7"), 1);
        assert_eq!(db.get_game_review(b).unwrap().unwrap().game.champion.as_deref(), Some("Ahri"));
    }

    #[test]
    fn the_lane_opponent_is_a_lookup_by_position() {
        use crate::live_client::events::{Scoreboard, ScoreboardPlayer};
        let player = |champion: &str, team: &str, position: Option<&str>, is_us: bool| ScoreboardPlayer {
            champion: champion.into(),
            team: team.into(),
            position: position.map(Into::into),
            is_us,
            ..Default::default()
        };
        let board = Scoreboard {
            players: vec![
                player("Lee Sin", "ORDER", Some("Jungle"), true),
                player("Ahri", "CHAOS", Some("Middle"), false),
                player("Vi", "CHAOS", Some("Jungle"), false),
            ],
            ..Default::default()
        };
        assert_eq!(lane_opponent(&board).as_deref(), Some("Vi"));

        let no_positions = Scoreboard {
            players: vec![player("Lee Sin", "ORDER", None, true), player("Vi", "CHAOS", None, false)],
            ..Default::default()
        };
        assert_eq!(lane_opponent(&no_positions), None);
    }
}
