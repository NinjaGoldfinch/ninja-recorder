//! VOD review (WS9): games, blocks, reviews, objectives and takeaways.
//!
//! The tables are migration 13 in `db/mod.rs`; `docs/data-model.md` has the
//! diagram. A review hangs off `games` rather than `recordings`, because a
//! recording row is deleted with its file and a review has to outlive it.

#[cfg(test)]
mod tests {
    use super::super::*;

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
}
