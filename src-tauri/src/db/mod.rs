//! SQLite-backed VOD library: `recordings` + `markers` tables, with
//! migrations from the first schema onward. DEVELOPMENT.md §4.
//!
//! MP4s on disk are the source of truth for video; these rows are
//! metadata. `reconcile` (submodule) is what keeps the two in sync when a
//! user touches the recordings folder directly.

pub mod reconcile;

use rusqlite::{params, Connection, OptionalExtension};
use rusqlite_migration::{Migrations, M};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{LazyLock, Mutex};

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("migration error: {0}")]
    Migration(#[from] rusqlite_migration::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// The file on disk is at a higher schema version than this build
    /// knows about — it was written by a newer build (in practice: another
    /// branch, or a downgrade). Migrations only run forward, so there is
    /// nothing this build can do with it. Detected explicitly rather than
    /// left to `rusqlite_migration`, whose `DatabaseTooFarAhead` surfaces
    /// as an opaque nested enum with no room to say which file or what to
    /// do about it.
    #[error(
        "database schema is v{found}, but this build only knows v{expected} —          it was created by a newer build of the app"
    )]
    SchemaTooNew { found: i64, expected: i64 },
}

/// The migration list, paired with its own length. Bundled rather than
/// kept as a separate constant so the "how many migrations does this build
/// know about" number can't drift from the list it describes — that number
/// is what `init` compares `PRAGMA user_version` against.
static MIGRATIONS: LazyLock<(Migrations<'static>, i64)> = LazyLock::new(|| {
    let migrations = vec![M::up(
        "
        CREATE TABLE recordings (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            path        TEXT NOT NULL UNIQUE,
            started_at  INTEGER NOT NULL, -- unix millis
            duration_s  REAL,
            game_id     INTEGER,
            queue       INTEGER,
            champion    TEXT,
            role        TEXT,
            win         INTEGER, -- 0/1, nullable (unknown until match data is fetched)
            kda_k       INTEGER,
            kda_d       INTEGER,
            kda_a       INTEGER,
            patch       TEXT,
            pinned      INTEGER NOT NULL DEFAULT 0,
            size_bytes  INTEGER NOT NULL DEFAULT 0
        );

        CREATE TABLE markers (
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
            recording_id  INTEGER NOT NULL REFERENCES recordings(id) ON DELETE CASCADE,
            game_time_s   REAL NOT NULL,
            video_time_s  REAL NOT NULL,
            kind          TEXT NOT NULL, -- kill|death|assist|dragon|baron|herald|turret|ace|first_blood|custom
            payload_json  TEXT NOT NULL DEFAULT '{}'
        );

        CREATE INDEX idx_markers_recording_id ON markers(recording_id);
        ",
    ), M::up(
        "
        -- Single-row settings table (Phase 9 / DEVELOPMENT.md §6). Defaults
        -- to 50 GiB / 30 days rather than unlimited: disk retention is a
        -- launch feature specifically because unbounded capture fills an
        -- SSD in weeks, so it should protect the user out of the box, not
        -- only once they find a settings screen.
        CREATE TABLE settings (
            id               INTEGER PRIMARY KEY CHECK (id = 1),
            max_total_bytes  INTEGER DEFAULT 53687091200, -- 50 GiB
            max_age_days     INTEGER DEFAULT 30
        );

        INSERT INTO settings (id) VALUES (1);
        ",
    ), M::up(
        "
        -- Per-poll time series behind the review timeline's advantage curve
        -- (1 Hz, so ~2100 rows for a 35-minute game — trivial for SQLite;
        -- downsampling happens at render time, not here).
        --
        -- The diffs are stored pre-signed from the recording player's point
        -- of view (positive = their team ahead) with `our_team` alongside,
        -- so the sign convention stays auditable rather than being an
        -- unwritten frontend assumption. `our_team` is NULL when the active
        -- player couldn't be matched in `allPlayers`; the UI renders that
        -- as a team-side-unknown state instead of a possibly-inverted line.
        --
        -- `gold_diff_est` is an ESTIMATE. The Live Client Data API exposes
        -- no per-player gold, so it's derived from summed item prices plus
        -- our own unspent gold (see `live_client::events::team_diff`).
        -- `kill_diff` and `cs_diff` are exact.
        CREATE TABLE samples (
            id             INTEGER PRIMARY KEY AUTOINCREMENT,
            recording_id   INTEGER NOT NULL REFERENCES recordings(id) ON DELETE CASCADE,
            game_time_s    REAL NOT NULL,
            video_time_s   REAL NOT NULL,
            our_team       TEXT,    -- ORDER|CHAOS, NULL if we couldn't be matched
            gold_diff_est  REAL,    -- signed, + = our team ahead. Estimated.
            kill_diff      INTEGER, -- signed, exact
            cs_diff        INTEGER, -- signed, exact
            our_gold       REAL,    -- activePlayer.currentGold, unspent
            our_level      INTEGER
        );

        CREATE INDEX idx_samples_recording_id ON samples(recording_id);
        ",
    ), M::up(
        "
        -- Generic key/value store for UI preferences (theme, default sort).
        -- Deliberately unseeded, unlike migration 2's single-row `settings`:
        -- retention has to protect the user out of the box, but a missing UI
        -- pref just means 'use the frontend default', which keeps adding a
        -- new pref a zero-migration change.
        CREATE TABLE settings_kv (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        ",
    )];
    let count = migrations.len() as i64;
    (Migrations::new(migrations), count)
});

#[derive(Debug, Clone, Default)]
pub struct NewRecording {
    pub path: String,
    pub started_at: i64,
    pub duration_s: Option<f64>,
    pub game_id: Option<i64>,
    pub queue: Option<i64>,
    pub champion: Option<String>,
    pub role: Option<String>,
    pub win: Option<bool>,
    pub kda_k: Option<i64>,
    pub kda_d: Option<i64>,
    pub kda_a: Option<i64>,
    pub patch: Option<String>,
    pub pinned: bool,
    pub size_bytes: i64,
}

#[derive(Debug, Clone)]
pub struct NewMarker {
    pub game_time_s: f64,
    pub video_time_s: f64,
    pub kind: String,
    pub payload_json: String,
}

/// One 1 Hz sample of the team-advantage series. Every metric is optional
/// because a poll can arrive before we've worked out which side we're on
/// (or at all, if the active player never matches an `allPlayers` entry).
#[derive(Debug, Clone, Default)]
pub struct NewSample {
    pub game_time_s: f64,
    pub video_time_s: f64,
    pub our_team: Option<String>,
    pub gold_diff_est: Option<f64>,
    pub kill_diff: Option<i64>,
    pub cs_diff: Option<i64>,
    pub our_gold: Option<f64>,
    pub our_level: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RecordingRow {
    pub id: i64,
    pub path: String,
    pub started_at: i64,
    pub duration_s: Option<f64>,
    pub game_id: Option<i64>,
    pub queue: Option<i64>,
    pub champion: Option<String>,
    pub role: Option<String>,
    pub win: Option<bool>,
    pub kda_k: Option<i64>,
    pub kda_d: Option<i64>,
    pub kda_a: Option<i64>,
    pub patch: Option<String>,
    pub pinned: bool,
    pub size_bytes: i64,
}

/// Disk retention policy (Phase 9 / DEVELOPMENT.md §6): `None` means that
/// dimension is unbounded. Mirrors the single-row `settings` table.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RetentionPolicy {
    pub max_total_bytes: Option<i64>,
    pub max_age_days: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MarkerRow {
    pub id: i64,
    pub recording_id: i64,
    pub game_time_s: f64,
    pub video_time_s: f64,
    pub kind: String,
    pub payload_json: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SampleRow {
    pub id: i64,
    pub recording_id: i64,
    pub game_time_s: f64,
    pub video_time_s: f64,
    pub our_team: Option<String>,
    pub gold_diff_est: Option<f64>,
    pub kill_diff: Option<i64>,
    pub cs_diff: Option<i64>,
    pub our_gold: Option<f64>,
    pub our_level: Option<i64>,
}

/// Shared row mapper for the `recordings` SELECT list used by
/// `list_recordings` and `get_recording` — the two must stay column-aligned,
/// so they read the tuple in one place.
fn row_to_recording(row: &rusqlite::Row) -> rusqlite::Result<RecordingRow> {
    Ok(RecordingRow {
        id: row.get(0)?,
        path: row.get(1)?,
        started_at: row.get(2)?,
        duration_s: row.get(3)?,
        game_id: row.get(4)?,
        queue: row.get(5)?,
        champion: row.get(6)?,
        role: row.get(7)?,
        win: row.get(8)?,
        kda_k: row.get(9)?,
        kda_d: row.get(10)?,
        kda_a: row.get(11)?,
        patch: row.get(12)?,
        pinned: row.get(13)?,
        size_bytes: row.get(14)?,
    })
}

pub struct Db {
    conn: Mutex<Connection>,
}

impl Db {
    pub fn open(path: &Path) -> Result<Self, DbError> {
        let mut conn = Connection::open(path)?;
        Self::init(&mut conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// `pub(crate)` rather than private: other modules' tests (e.g.
    /// `state_machine::supervisor`) need this too, but it must never be
    /// reachable outside `#[cfg(test)]` builds.
    #[cfg(test)]
    pub(crate) fn open_in_memory() -> Result<Self, DbError> {
        let mut conn = Connection::open_in_memory()?;
        Self::init(&mut conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Raw connection access for the dev portal's SQL console and table
    /// browser (`dev::sql`). Deliberately feature-gated rather than
    /// `pub(crate)` outright: everything reachable through this bypasses
    /// the typed `NewRecording`/`NewMarker` API, the migrations, and the
    /// `path` upsert rule above, so it must not exist at all in a shipped
    /// build. Panics on a poisoned lock, matching every other method here.
    #[cfg(feature = "devtools")]
    pub(crate) fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap()
    }

    fn init(conn: &mut Connection) -> Result<(), DbError> {
        conn.pragma_update(None, "foreign_keys", "ON")?;

        // `rusqlite_migration` would catch this too, but only as
        // `MigrationDefinition(DatabaseTooFarAhead)` — which reaches the
        // user as a Rust panic and a backtrace, from inside Tauri's setup
        // hook. Checking first lets the failure carry both version numbers
        // and lets `lib.rs` say what to do about it.
        let (migrations, expected) = &*MIGRATIONS;
        let found: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if found > *expected {
            return Err(DbError::SchemaTooNew {
                found,
                expected: *expected,
            });
        }

        migrations.to_latest(conn)?;
        Ok(())
    }

    /// Upserts on `path` rather than a plain `INSERT`: `reconcile` (folder
    /// scan, run at startup and on-demand) can't tell an in-progress
    /// recording's not-yet-finalized file apart from a genuinely untracked
    /// one, so it may already have imported this exact path as an
    /// "unknown recording" by the time the real finalize gets here. A
    /// plain `INSERT` would then fail the `UNIQUE` constraint on `path`
    /// and silently drop the DB row (`stop_recording`'s `recording_id:
    /// None` case) even though the recording itself succeeded. The real
    /// finalize data should win over reconcile's guessed one either way.
    pub fn insert_recording(&self, new: &NewRecording) -> Result<i64, DbError> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "INSERT INTO recordings
                (path, started_at, duration_s, game_id, queue, champion, role,
                 win, kda_k, kda_d, kda_a, patch, pinned, size_bytes)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
             ON CONFLICT(path) DO UPDATE SET
                started_at = excluded.started_at,
                duration_s = excluded.duration_s,
                game_id    = excluded.game_id,
                queue      = excluded.queue,
                champion   = excluded.champion,
                role       = excluded.role,
                win        = excluded.win,
                kda_k      = excluded.kda_k,
                kda_d      = excluded.kda_d,
                kda_a      = excluded.kda_a,
                patch      = excluded.patch,
                pinned     = excluded.pinned,
                size_bytes = excluded.size_bytes
             RETURNING id",
            params![
                new.path,
                new.started_at,
                new.duration_s,
                new.game_id,
                new.queue,
                new.champion,
                new.role,
                new.win,
                new.kda_k,
                new.kda_d,
                new.kda_a,
                new.patch,
                new.pinned,
                new.size_bytes,
            ],
            |row| row.get(0),
        )
        .map_err(DbError::from)
    }

    /// Inserts all `markers` for `recording_id` in one transaction.
    pub fn insert_markers(&self, recording_id: i64, markers: &[NewMarker]) -> Result<(), DbError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO markers (recording_id, game_time_s, video_time_s, kind, payload_json)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;
            for m in markers {
                stmt.execute(params![
                    recording_id,
                    m.game_time_s,
                    m.video_time_s,
                    m.kind,
                    m.payload_json
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Inserts all `samples` for `recording_id` in one transaction —
    /// same shape as `insert_markers`, but this runs with ~2100 rows on a
    /// normal game, so the single-transaction batching matters more here.
    pub fn insert_samples(&self, recording_id: i64, samples: &[NewSample]) -> Result<(), DbError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO samples
                    (recording_id, game_time_s, video_time_s, our_team,
                     gold_diff_est, kill_diff, cs_diff, our_gold, our_level)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )?;
            for s in samples {
                stmt.execute(params![
                    recording_id,
                    s.game_time_s,
                    s.video_time_s,
                    s.our_team,
                    s.gold_diff_est,
                    s.kill_diff,
                    s.cs_diff,
                    s.our_gold,
                    s.our_level,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Advantage-curve samples for one recording, ordered by position in
    /// the video — what the review timeline's graph plots.
    pub fn get_samples(&self, recording_id: i64) -> Result<Vec<SampleRow>, DbError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, recording_id, game_time_s, video_time_s, our_team,
                    gold_diff_est, kill_diff, cs_diff, our_gold, our_level
             FROM samples WHERE recording_id = ?1 ORDER BY video_time_s ASC",
        )?;
        let rows = stmt.query_map([recording_id], |row| {
            Ok(SampleRow {
                id: row.get(0)?,
                recording_id: row.get(1)?,
                game_time_s: row.get(2)?,
                video_time_s: row.get(3)?,
                our_team: row.get(4)?,
                gold_diff_est: row.get(5)?,
                kill_diff: row.get(6)?,
                cs_diff: row.get(7)?,
                our_gold: row.get(8)?,
                our_level: row.get(9)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(DbError::from)
    }

    pub fn list_recordings(&self) -> Result<Vec<RecordingRow>, DbError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, path, started_at, duration_s, game_id, queue, champion, role,
                    win, kda_k, kda_d, kda_a, patch, pinned, size_bytes
             FROM recordings ORDER BY started_at DESC",
        )?;
        let rows = stmt.query_map([], row_to_recording)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(DbError::from)
    }

    pub fn find_by_path(&self, path: &str) -> Result<Option<i64>, DbError> {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT id FROM recordings WHERE path = ?1", [path], |r| {
            r.get(0)
        })
        .optional()
        .map_err(DbError::from)
    }

    /// Markers for one recording, ordered by position in the video —
    /// what the review timeline (Phase 5) renders.
    pub fn get_markers(&self, recording_id: i64) -> Result<Vec<MarkerRow>, DbError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, recording_id, game_time_s, video_time_s, kind, payload_json
             FROM markers WHERE recording_id = ?1 ORDER BY video_time_s ASC",
        )?;
        let rows = stmt.query_map([recording_id], |row| {
            Ok(MarkerRow {
                id: row.get(0)?,
                recording_id: row.get(1)?,
                game_time_s: row.get(2)?,
                video_time_s: row.get(3)?,
                kind: row.get(4)?,
                payload_json: row.get(5)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(DbError::from)
    }

    pub fn delete_recording(&self, id: i64) -> Result<(), DbError> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM recordings WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn set_pinned(&self, id: i64, pinned: bool) -> Result<(), DbError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE recordings SET pinned = ?1 WHERE id = ?2",
            params![pinned, id],
        )?;
        Ok(())
    }

    /// Sum of `size_bytes` across every recording, pinned or not — this is
    /// disk *usage*, which pinned files still count toward even though
    /// they're exempt from retention deletion.
    pub fn total_size_bytes(&self) -> Result<i64, DbError> {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT COALESCE(SUM(size_bytes), 0) FROM recordings", [], |r| {
            r.get(0)
        })
        .map_err(DbError::from)
    }

    pub fn get_retention_policy(&self) -> Result<RetentionPolicy, DbError> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT max_total_bytes, max_age_days FROM settings WHERE id = 1",
            [],
            |row| {
                Ok(RetentionPolicy {
                    max_total_bytes: row.get(0)?,
                    max_age_days: row.get(1)?,
                })
            },
        )
        .map_err(DbError::from)
    }

    pub fn set_retention_policy(&self, policy: &RetentionPolicy) -> Result<(), DbError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE settings SET max_total_bytes = ?1, max_age_days = ?2 WHERE id = 1",
            params![policy.max_total_bytes, policy.max_age_days],
        )?;
        Ok(())
    }

    /// One recording by id. `find_by_path`'s counterpart — used by the
    /// user-initiated delete, which needs the row's `path` and `size_bytes`
    /// before it can remove the file.
    pub fn get_recording(&self, id: i64) -> Result<Option<RecordingRow>, DbError> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, path, started_at, duration_s, game_id, queue, champion, role,
                    win, kda_k, kda_d, kda_a, patch, pinned, size_bytes
             FROM recordings WHERE id = ?1",
            [id],
            row_to_recording,
        )
        .optional()
        .map_err(DbError::from)
    }

    /// Every UI preference in one round trip — the frontend reads the whole
    /// set once at boot, so N separate `get_ui_pref` calls would just be
    /// N times the IPC for the same data.
    pub fn get_ui_prefs(&self) -> Result<HashMap<String, String>, DbError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT key, value FROM settings_kv")?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect::<Result<HashMap<_, _>, _>>()
            .map_err(DbError::from)
    }

    pub fn set_ui_pref(&self, key: &str, value: &str) -> Result<(), DbError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO settings_kv (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A database written by a *newer* build must be refused with a
    /// diagnosable error rather than the library's opaque
    /// `DatabaseTooFarAhead`. This is reachable just by switching to an
    /// older branch, and before it was handled it aborted the whole app
    /// from inside Tauri's setup hook.
    #[test]
    fn refuses_a_schema_from_a_newer_build() {
        let mut conn = Connection::open_in_memory().unwrap();
        Db::init(&mut conn).unwrap();

        let (_, expected) = &*MIGRATIONS;
        let ahead = *expected + 1;
        conn.pragma_update(None, "user_version", ahead).unwrap();

        match Db::init(&mut conn) {
            Err(DbError::SchemaTooNew { found, expected: known }) => {
                assert_eq!(found, ahead);
                assert_eq!(known, *expected);
            }
            other => panic!("expected SchemaTooNew, got {other:?}"),
        }
    }

    /// The check is one-sided: an older file is exactly what migrations
    /// are for, and must still be brought forward.
    #[test]
    fn still_migrates_a_database_from_an_older_build() {
        let mut conn = Connection::open_in_memory().unwrap();
        Db::init(&mut conn).unwrap();

        let (_, expected) = &*MIGRATIONS;
        assert_eq!(
            conn.query_row::<i64, _, _>("PRAGMA user_version", [], |r| r.get(0))
                .unwrap(),
            *expected,
            "a fresh database should land on the newest schema"
        );

        // Re-running against an already-current file is also a no-op.
        Db::init(&mut conn).unwrap();
    }

    fn marker(kind: &str, game_time_s: f64) -> NewMarker {
        NewMarker {
            game_time_s,
            video_time_s: game_time_s + 5.0,
            kind: kind.to_string(),
            payload_json: "{}".to_string(),
        }
    }

    #[test]
    fn insert_and_list_recording() {
        let db = Db::open_in_memory().unwrap();
        let id = db
            .insert_recording(&NewRecording {
                path: "/recordings/one.mp4".into(),
                started_at: 1000,
                size_bytes: 12345,
                ..Default::default()
            })
            .unwrap();

        let rows = db.list_recordings().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, id);
        assert_eq!(rows[0].path, "/recordings/one.mp4");
        assert_eq!(rows[0].size_bytes, 12345);
        assert!(!rows[0].pinned);
        assert_eq!(rows[0].champion, None);
    }

    #[test]
    fn list_recordings_orders_newest_first() {
        let db = Db::open_in_memory().unwrap();
        db.insert_recording(&NewRecording {
            path: "/a.mp4".into(),
            started_at: 100,
            ..Default::default()
        })
        .unwrap();
        db.insert_recording(&NewRecording {
            path: "/b.mp4".into(),
            started_at: 200,
            ..Default::default()
        })
        .unwrap();

        let rows = db.list_recordings().unwrap();
        assert_eq!(rows[0].path, "/b.mp4");
        assert_eq!(rows[1].path, "/a.mp4");
    }

    /// Reconcile (folder-scan) and a recording's own finalize can both
    /// try to insert the same path — reconcile can't distinguish a
    /// not-yet-finalized in-progress file from a genuinely untracked one,
    /// so it may import it first. The later insert must win with its
    /// (more authoritative) data instead of erroring and losing the row
    /// finalize needs to attach markers to.
    #[test]
    fn duplicate_path_upserts_instead_of_erroring() {
        let db = Db::open_in_memory().unwrap();
        let first_id = db
            .insert_recording(&NewRecording {
                path: "/dup.mp4".into(),
                started_at: 1,
                champion: None,
                ..Default::default()
            })
            .unwrap();

        let second_id = db
            .insert_recording(&NewRecording {
                path: "/dup.mp4".into(),
                started_at: 2,
                champion: Some("Ahri".into()),
                ..Default::default()
            })
            .unwrap();

        assert_eq!(first_id, second_id, "upsert should keep the same row id");
        let rows = db.list_recordings().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].started_at, 2);
        assert_eq!(rows[0].champion, Some("Ahri".to_string()));
    }

    #[test]
    fn insert_and_count_markers() {
        let db = Db::open_in_memory().unwrap();
        let id = db
            .insert_recording(&NewRecording {
                path: "/game.mp4".into(),
                started_at: 1,
                ..Default::default()
            })
            .unwrap();

        db.insert_markers(id, &[marker("kill", 10.0), marker("death", 20.0)])
            .unwrap();

        let conn = db.conn.lock().unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM markers WHERE recording_id = ?1",
                [id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn get_markers_returns_them_ordered_by_video_time() {
        let db = Db::open_in_memory().unwrap();
        let id = db
            .insert_recording(&NewRecording {
                path: "/game.mp4".into(),
                started_at: 1,
                ..Default::default()
            })
            .unwrap();

        // Inserted out of order — get_markers must sort them.
        db.insert_markers(id, &[marker("death", 20.0), marker("kill", 10.0)])
            .unwrap();

        let markers = db.get_markers(id).unwrap();
        assert_eq!(markers.len(), 2);
        assert_eq!(markers[0].kind, "kill");
        assert_eq!(markers[0].game_time_s, 10.0);
        assert_eq!(markers[1].kind, "death");
    }

    #[test]
    fn get_markers_for_recording_with_none_is_empty() {
        let db = Db::open_in_memory().unwrap();
        let id = db
            .insert_recording(&NewRecording {
                path: "/quiet-game.mp4".into(),
                started_at: 1,
                ..Default::default()
            })
            .unwrap();

        assert!(db.get_markers(id).unwrap().is_empty());
    }

    #[test]
    fn deleting_recording_cascades_to_its_markers() {
        let db = Db::open_in_memory().unwrap();
        let id = db
            .insert_recording(&NewRecording {
                path: "/game.mp4".into(),
                started_at: 1,
                ..Default::default()
            })
            .unwrap();
        db.insert_markers(id, &[marker("kill", 10.0)]).unwrap();

        db.delete_recording(id).unwrap();

        let conn = db.conn.lock().unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM markers", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn find_by_path_distinguishes_present_and_absent() {
        let db = Db::open_in_memory().unwrap();
        db.insert_recording(&NewRecording {
            path: "/known.mp4".into(),
            started_at: 1,
            ..Default::default()
        })
        .unwrap();

        assert!(db.find_by_path("/known.mp4").unwrap().is_some());
        assert!(db.find_by_path("/unknown.mp4").unwrap().is_none());
    }

    #[test]
    fn get_recording_returns_the_whole_row_or_none() {
        let db = Db::open_in_memory().unwrap();
        let id = db
            .insert_recording(&NewRecording {
                path: "/vod.mp4".into(),
                started_at: 42,
                champion: Some("Ahri".into()),
                size_bytes: 1234,
                ..Default::default()
            })
            .unwrap();

        let row = db.get_recording(id).unwrap().expect("row should exist");
        assert_eq!(row.path, "/vod.mp4");
        assert_eq!(row.champion.as_deref(), Some("Ahri"));
        assert_eq!(row.size_bytes, 1234);

        assert!(db.get_recording(id + 999).unwrap().is_none());
    }

    #[test]
    fn ui_pref_round_trips_and_overwrites() {
        let db = Db::open_in_memory().unwrap();

        db.set_ui_pref("theme", "dark").unwrap();
        let prefs = db.get_ui_prefs().unwrap();
        assert_eq!(prefs.get("theme").map(String::as_str), Some("dark"));

        // Upsert, not a second row — the frontend saves on every toggle.
        db.set_ui_pref("theme", "light").unwrap();
        let prefs = db.get_ui_prefs().unwrap();
        assert_eq!(prefs.get("theme").map(String::as_str), Some("light"));
        assert_eq!(prefs.len(), 1);
    }

    #[test]
    fn missing_ui_pref_is_absent_not_an_error() {
        let db = Db::open_in_memory().unwrap();
        assert!(!db.get_ui_prefs().unwrap().contains_key("never-set"));
    }

    #[test]
    fn get_ui_prefs_returns_every_pref_and_starts_empty() {
        let db = Db::open_in_memory().unwrap();
        // Migration 4 deliberately seeds nothing: a missing pref means
        // "use the frontend default".
        assert!(db.get_ui_prefs().unwrap().is_empty());

        db.set_ui_pref("theme", "dark").unwrap();
        db.set_ui_pref("defaultSort", "champion").unwrap();

        let prefs = db.get_ui_prefs().unwrap();
        assert_eq!(prefs.get("theme").map(String::as_str), Some("dark"));
        assert_eq!(prefs.get("defaultSort").map(String::as_str), Some("champion"));
    }

    #[test]
    fn set_pinned_updates_the_row() {
        let db = Db::open_in_memory().unwrap();
        let id = db
            .insert_recording(&NewRecording {
                path: "/game.mp4".into(),
                started_at: 1,
                ..Default::default()
            })
            .unwrap();

        db.set_pinned(id, true).unwrap();
        assert!(db.list_recordings().unwrap()[0].pinned);

        db.set_pinned(id, false).unwrap();
        assert!(!db.list_recordings().unwrap()[0].pinned);
    }

    #[test]
    fn total_size_bytes_sums_across_recordings() {
        let db = Db::open_in_memory().unwrap();
        db.insert_recording(&NewRecording {
            path: "/a.mp4".into(),
            started_at: 1,
            size_bytes: 100,
            ..Default::default()
        })
        .unwrap();
        db.insert_recording(&NewRecording {
            path: "/b.mp4".into(),
            started_at: 2,
            size_bytes: 250,
            ..Default::default()
        })
        .unwrap();

        assert_eq!(db.total_size_bytes().unwrap(), 350);
    }

    #[test]
    fn retention_policy_defaults_and_round_trips() {
        let db = Db::open_in_memory().unwrap();

        // Seeded defaults from the migration — 50 GiB / 30 days, not
        // unlimited (see the migration's comment for why).
        let defaults = db.get_retention_policy().unwrap();
        assert_eq!(defaults.max_total_bytes, Some(53_687_091_200));
        assert_eq!(defaults.max_age_days, Some(30));

        let updated = RetentionPolicy {
            max_total_bytes: None,
            max_age_days: Some(7),
        };
        db.set_retention_policy(&updated).unwrap();
        assert_eq!(db.get_retention_policy().unwrap(), updated);
    }

    fn sample(video_time_s: f64, gold: f64, kills: i64) -> NewSample {
        NewSample {
            game_time_s: video_time_s - 5.0,
            video_time_s,
            our_team: Some("ORDER".into()),
            gold_diff_est: Some(gold),
            kill_diff: Some(kills),
            cs_diff: Some(0),
            our_gold: Some(450.0),
            our_level: Some(11),
        }
    }

    fn recording_with_samples(db: &Db, samples: &[NewSample]) -> i64 {
        let id = db
            .insert_recording(&NewRecording {
                path: "/game.mp4".into(),
                started_at: 1,
                ..Default::default()
            })
            .unwrap();
        db.insert_samples(id, samples).unwrap();
        id
    }

    #[test]
    fn get_samples_returns_them_ordered_by_video_time() {
        let db = Db::open_in_memory().unwrap();
        // Inserted out of order — get_samples must sort them, since the
        // graph renderer walks the series assuming monotonic time.
        let id = recording_with_samples(
            &db,
            &[sample(30.0, 900.0, 2), sample(10.0, 100.0, 0), sample(20.0, -400.0, -1)],
        );

        let rows = db.get_samples(id).unwrap();
        let times: Vec<f64> = rows.iter().map(|r| r.video_time_s).collect();
        assert_eq!(times, vec![10.0, 20.0, 30.0]);
    }

    /// Negative diffs are the whole point of the metric — a column typed or
    /// bound wrongly would clamp "behind" to zero and the curve would only
    /// ever show good news.
    #[test]
    fn sample_diffs_round_trip_with_their_sign_intact() {
        let db = Db::open_in_memory().unwrap();
        let id = recording_with_samples(&db, &[sample(10.0, -2750.5, -3)]);

        let row = &db.get_samples(id).unwrap()[0];
        assert_eq!(row.gold_diff_est, Some(-2750.5));
        assert_eq!(row.kill_diff, Some(-3));
        assert_eq!(row.our_team, Some("ORDER".to_string()));
        assert_eq!(row.our_gold, Some(450.0));
    }

    /// A poll where the active player couldn't be matched in `allPlayers`
    /// still gets a row, with the metrics NULL rather than a guessed side.
    #[test]
    fn sample_with_unknown_team_stores_nulls() {
        let db = Db::open_in_memory().unwrap();
        let id = recording_with_samples(
            &db,
            &[NewSample {
                game_time_s: 5.0,
                video_time_s: 10.0,
                ..Default::default()
            }],
        );

        let row = &db.get_samples(id).unwrap()[0];
        assert_eq!(row.our_team, None);
        assert_eq!(row.gold_diff_est, None);
        assert_eq!(row.kill_diff, None);
    }

    #[test]
    fn get_samples_for_recording_with_none_is_empty() {
        let db = Db::open_in_memory().unwrap();
        let id = recording_with_samples(&db, &[]);
        assert!(db.get_samples(id).unwrap().is_empty());
    }

    #[test]
    fn deleting_recording_cascades_to_its_samples() {
        let db = Db::open_in_memory().unwrap();
        let id = recording_with_samples(&db, &[sample(10.0, 100.0, 1)]);

        db.delete_recording(id).unwrap();

        let conn = db.conn.lock().unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM samples", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

}
