//! SQLite-backed VOD library: `recordings` + `markers` tables, with
//! migrations from the first schema onward. DEVELOPMENT.md §4.
//!
//! MP4s on disk are the source of truth for video; these rows are
//! metadata. `reconcile` (submodule) is what keeps the two in sync when a
//! user touches the recordings folder directly.

pub mod reconcile;

use rusqlite::{params, Connection, OptionalExtension};
use rusqlite_migration::{Migrations, M};
use serde::Serialize;
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
}

static MIGRATIONS: LazyLock<Migrations<'static>> = LazyLock::new(|| {
    Migrations::new(vec![M::up(
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
    )])
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

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MarkerRow {
    pub id: i64,
    pub recording_id: i64,
    pub game_time_s: f64,
    pub video_time_s: f64,
    pub kind: String,
    pub payload_json: String,
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

    fn init(conn: &mut Connection) -> Result<(), DbError> {
        conn.pragma_update(None, "foreign_keys", "ON")?;
        MIGRATIONS.to_latest(conn)?;
        Ok(())
    }

    pub fn insert_recording(&self, new: &NewRecording) -> Result<i64, DbError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO recordings
                (path, started_at, duration_s, game_id, queue, champion, role,
                 win, kda_k, kda_d, kda_a, patch, pinned, size_bytes)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
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
        )?;
        Ok(conn.last_insert_rowid())
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

    pub fn list_recordings(&self) -> Result<Vec<RecordingRow>, DbError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, path, started_at, duration_s, game_id, queue, champion, role,
                    win, kda_k, kda_d, kda_a, patch, pinned, size_bytes
             FROM recordings ORDER BY started_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
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
        })?;
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(rows[0].pinned, false);
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

    #[test]
    fn duplicate_path_is_rejected() {
        let db = Db::open_in_memory().unwrap();
        db.insert_recording(&NewRecording {
            path: "/dup.mp4".into(),
            started_at: 1,
            ..Default::default()
        })
        .unwrap();

        let result = db.insert_recording(&NewRecording {
            path: "/dup.mp4".into(),
            started_at: 2,
            ..Default::default()
        });
        assert!(result.is_err());
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
}
