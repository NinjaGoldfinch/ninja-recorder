//! Folder-scan reconciliation: a DB row whose file is gone gets removed;
//! a video file with no DB row gets imported as an "unknown recording".
//! DEVELOPMENT.md §4 — "the library must survive users touching the
//! folder." Run once at app startup and available on demand (dev panel /
//! future UI rescan button).
//!
//! `recover_unfinished` is the other half, and runs **at daemon startup
//! only**. A recording that was in flight when a daemon died leaves a row
//! with no `finished_at`, which `list_recordings` hides and `reconcile`'s
//! own orphan sweep therefore cannot see; its file is skipped by the import
//! pass too, because `find_by_path` finds the row. Without a pass that
//! finishes those rows from the file, a killed recording would be
//! *invisible* rather than merely stripped of its metadata, which is worse
//! than the bug #150 set out to fix.

use super::{Db, DbError, NewRecording};
use serde::Serialize;
use std::path::Path;

const VIDEO_EXTENSIONS: &[&str] = &["mp4", "mkv"];

#[derive(Debug, Clone, Default, PartialEq, Serialize, ts_rs::TS)]
pub struct ReconcileReport {
    pub orphans_removed: usize,
    pub imported: usize,
}

/// What `recover_unfinished` did. Both numbers are normally zero: an
/// unfinished row at startup means the previous daemon did not get to
/// finalize.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RecoveryReport {
    /// Rows finished from their file, keeping the markers written during
    /// the game.
    pub recovered: usize,
    /// Rows whose file is gone, deleted. A recording that never wrote a
    /// frame, or whose file the user removed before the daemon came back.
    pub abandoned_removed: usize,
}

/// Finishes the rows a dead daemon left open, from what their files can be
/// made to say.
///
/// **Startup only, and that is a correctness requirement, not a
/// preference.** An in-progress recording is indistinguishable here from an
/// abandoned one: both are a row with no `finished_at` and a file on disk.
/// Running this while the supervisor is recording would finish the row it is
/// still writing, putting a half-written file in the library and letting
/// retention delete it. The daemon calls this before it can start recording,
/// which is what makes the question safe to not ask.
///
/// Only the columns a file can answer for are written. The champion, the
/// KDA and the outcome died with the daemon that was polling for them, and
/// a default is a confident wrong answer where NULL is a true one. The
/// markers stay untouched: they are what this whole path exists to keep.
pub fn recover_unfinished(db: &Db, ffmpeg: Option<&Path>) -> Result<RecoveryReport, DbError> {
    let mut report = RecoveryReport::default();

    for row in db.unfinished_recordings()? {
        let path = Path::new(&row.path);
        let Ok(metadata) = path.metadata() else {
            // No file, so nothing to finish and nothing to show. This is
            // also the only sweep that can reach these rows, since
            // `reconcile`'s runs off `list_recordings`, which hides them.
            db.delete_recording(row.id)?;
            report.abandoned_removed += 1;
            continue;
        };

        // The session's clock is gone, so the file is the only source of a
        // duration. `None` stays NULL, exactly as it does for an import that
        // could not be probed.
        let duration_s = ffmpeg.and_then(|ffmpeg| crate::probe::duration_s(ffmpeg, path));
        // The file's mtime, not now: the recording ended when the daemon
        // died, which may have been days ago, and `finished_at` ordering a
        // recovered recording above everything since would be a lie the
        // library sorts on.
        let finished_at = file_modified_millis(&metadata);
        db.recover_recording(row.id, duration_s, metadata.len() as i64, finished_at)?;
        report.recovered += 1;
    }

    Ok(report)
}

/// `ffmpeg` is the bundled (or locally installed) binary, if this build has
/// one. It is used to read a duration out of each file being imported, and
/// `None` simply leaves that column NULL — see `probe`.
pub fn reconcile(
    db: &Db,
    recordings_dir: &Path,
    ffmpeg: Option<&Path>,
) -> Result<ReconcileReport, DbError> {
    let mut report = ReconcileReport::default();

    for row in db.list_recordings()? {
        if !Path::new(&row.path).exists() {
            db.delete_recording(row.id)?;
            report.orphans_removed += 1;
        }
    }

    if recordings_dir.exists() {
        for entry in std::fs::read_dir(recordings_dir)? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_file() || !is_video_file(&path) {
                continue;
            }

            let path_str = path.to_string_lossy().to_string();
            if db.find_by_path(&path_str)?.is_some() {
                continue;
            }

            let metadata = entry.metadata()?;
            // Once per file, on the import path only: the `find_by_path`
            // skip above means a rescan of a settled folder spawns nothing.
            // A first run against a large existing folder is the case that
            // costs — one ffmpeg per file, and startup reconcile is inline
            // (`lib.rs`'s `setup`), so it is startup latency.
            //
            // A recording in flight no longer reaches here: its row is
            // written when capture starts, so `find_by_path` above finds it
            // and this loop skips the file (#150). What is left is a file the
            // user put in the folder, or one whose row was deleted.
            //
            // The race is narrower than it was but not gone. A recording that
            // began before this version shipped has no row, and a start-insert
            // that failed leaves none either; in both cases the growing file
            // is still importable here, and a finalize landing between the
            // lookup and the insert leaves the probed duration in place. That
            // costs a slightly short length on one recording, and
            // `finish_recording` deletes the imported row when it lands.
            let duration_s = ffmpeg.and_then(|ffmpeg| crate::probe::duration_s(ffmpeg, &path));
            let modified = file_modified_millis(&metadata);
            db.insert_recording(&NewRecording {
                path: path_str,
                started_at: modified,
                size_bytes: metadata.len() as i64,
                duration_s,
                // **Imported files are finished, and saying so is not
                // optional** (#150). A row with no `finished_at` is hidden
                // from the library, so leaving this `None` would import every
                // untracked recording straight into invisibility.
                //
                // The file's own mtime is the only end time available. It is
                // the same value `started_at` gets, which is honest about how
                // little is known: a file found on disk has no session behind
                // it to ask.
                finished_at: Some(modified),
                ..Default::default()
            })?;
            report.imported += 1;
        }
    }

    Ok(report)
}

fn is_video_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| VIDEO_EXTENSIONS.contains(&ext.to_lowercase().as_str()))
        .unwrap_or(false)
}

fn file_modified_millis(metadata: &std::fs::Metadata) -> i64 {
    use std::time::UNIX_EPOCH;
    metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ninja-recorder-reconcile-test-{name}-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn removes_db_row_whose_file_is_gone() {
        let db = Db::open_temporary().unwrap();
        let dir = temp_dir("orphan");
        db.insert_recording(&NewRecording {
            path: dir.join("gone.mp4").to_string_lossy().to_string(),
            started_at: 1,
            finished_at: Some(1),
            ..Default::default()
        })
        .unwrap();

        let report = reconcile(&db, &dir, None).unwrap();
        assert_eq!(report.orphans_removed, 1);
        assert_eq!(report.imported, 0);
        assert!(db.list_recordings().unwrap().is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn imports_untracked_video_file() {
        let db = Db::open_temporary().unwrap();
        let dir = temp_dir("import");
        std::fs::write(dir.join("untracked.mp4"), b"fake video bytes").unwrap();

        let report = reconcile(&db, &dir, None).unwrap();
        assert_eq!(report.orphans_removed, 0);
        assert_eq!(report.imported, 1);

        let rows = db.list_recordings().unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].path.ends_with("untracked.mp4"));
        assert!(rows[0].champion.is_none(), "imported rows carry no match metadata");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn import_without_an_ffmpeg_leaves_duration_unknown() {
        let db = Db::open_temporary().unwrap();
        let dir = temp_dir("no-ffmpeg");
        std::fs::write(dir.join("untracked.mp4"), b"fake video bytes").unwrap();

        assert_eq!(reconcile(&db, &dir, None).unwrap().imported, 1);
        assert_eq!(db.list_recordings().unwrap()[0].duration_s, None);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_ffmpeg_that_cannot_be_run_still_imports_the_file() {
        // The failure-tolerance the issue asks for: reconcile must not
        // start failing over a cosmetic column. A path that isn't a binary
        // stands in for every way the probe can come back empty.
        let db = Db::open_temporary().unwrap();
        let dir = temp_dir("broken-ffmpeg");
        std::fs::write(dir.join("untracked.mp4"), b"fake video bytes").unwrap();

        let report = reconcile(&db, &dir, Some(Path::new("/definitely/not/ffmpeg"))).unwrap();
        assert_eq!(report.imported, 1);
        assert_eq!(db.list_recordings().unwrap()[0].duration_s, None);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn ignores_non_video_files() {
        let db = Db::open_temporary().unwrap();
        let dir = temp_dir("ignore");
        std::fs::write(dir.join("notes.txt"), b"not a video").unwrap();

        let report = reconcile(&db, &dir, None).unwrap();
        assert_eq!(report.imported, 0);
        assert!(db.list_recordings().unwrap().is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn tracked_file_is_left_alone() {
        let db = Db::open_temporary().unwrap();
        let dir = temp_dir("tracked");
        let file_path = dir.join("known.mp4");
        std::fs::write(&file_path, b"fake video bytes").unwrap();
        db.insert_recording(&NewRecording {
            path: file_path.to_string_lossy().to_string(),
            started_at: 1,
            champion: Some("Ahri".into()),
            finished_at: Some(1),
            ..Default::default()
        })
        .unwrap();

        let report = reconcile(&db, &dir, None).unwrap();
        assert_eq!(report, ReconcileReport::default());

        let rows = db.list_recordings().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].champion, Some("Ahri".to_string()));

        std::fs::remove_dir_all(&dir).ok();
    }

    // --- Recovering what a killed daemon left open (#150) -----------------

    /// The acceptance test for the second half of #150. A daemon that died
    /// mid-recording left a row with no `finished_at`, holding the markers
    /// written during the game. Startup has to finish it from the file, and
    /// **keep the markers**, which are the whole point of the row existing.
    #[test]
    fn recovery_finishes_an_abandoned_row_and_keeps_its_markers() {
        let db = Db::open_temporary().unwrap();
        let dir = temp_dir("recover");
        let file = dir.join("recording-1.mp4");
        std::fs::write(&file, b"partial but playable").unwrap();

        let id = db
            .begin_recording(&file.to_string_lossy(), 1_000)
            .unwrap();
        db.insert_markers(
            id,
            &[crate::db::NewMarker {
                game_time_s: 210.5,
                video_time_s: 210.5,
                kind: "kill".into(),
                payload_json: "{}".into(),
            }],
        )
        .unwrap();

        assert!(db.list_recordings().unwrap().is_empty(), "hidden while unfinished");

        let report = recover_unfinished(&db, None).unwrap();
        assert_eq!(report.recovered, 1);
        assert_eq!(report.abandoned_removed, 0);

        let rows = db.list_recordings().unwrap();
        assert_eq!(rows.len(), 1, "it is a library entry now");
        assert_eq!(rows[0].id, id);
        assert_eq!(rows[0].size_bytes, "partial but playable".len() as i64);
        assert_eq!(
            db.get_markers(id).unwrap().len(),
            1,
            "the markers are what survived the kill; recovery must not touch them"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Nothing to show and nothing to keep. This is also the only sweep that
    /// can reach such a row: `reconcile`'s orphan pass runs off
    /// `list_recordings`, which hides unfinished rows by design.
    #[test]
    fn recovery_deletes_an_abandoned_row_whose_file_is_gone() {
        let db = Db::open_temporary().unwrap();
        let dir = temp_dir("recover-no-file");
        let missing = dir.join("never-written.mp4");

        db.begin_recording(&missing.to_string_lossy(), 1_000).unwrap();

        let report = recover_unfinished(&db, None).unwrap();
        assert_eq!(report.abandoned_removed, 1);
        assert_eq!(report.recovered, 0);
        assert!(db.unfinished_recordings().unwrap().is_empty());
        assert!(db.list_recordings().unwrap().is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A finished row is not this pass's business, and rewriting its
    /// duration from a probe would undo the session clock's better answer.
    #[test]
    fn recovery_leaves_finished_rows_alone() {
        let db = Db::open_temporary().unwrap();
        let dir = temp_dir("recover-finished");
        let file = dir.join("done.mp4");
        std::fs::write(&file, b"fake video bytes").unwrap();

        db.insert_recording(&NewRecording {
            path: file.to_string_lossy().to_string(),
            started_at: 1,
            duration_s: Some(1234.5),
            champion: Some("Ahri".into()),
            finished_at: Some(2),
            ..Default::default()
        })
        .unwrap();

        assert_eq!(recover_unfinished(&db, None).unwrap(), RecoveryReport::default());
        let rows = db.list_recordings().unwrap();
        assert_eq!(rows[0].duration_s, Some(1234.5));
        assert_eq!(rows[0].champion.as_deref(), Some("Ahri"));

        std::fs::remove_dir_all(&dir).ok();
    }

    /// The other thing the start-insert buys. `reconcile` used to import an
    /// in-progress recording's growing file as an "unknown recording",
    /// because it could not tell one from a file the user had dropped in.
    /// Now the row exists, `find_by_path` finds it, and the scan skips it.
    #[test]
    fn an_in_progress_recording_is_not_imported_as_an_unknown_recording() {
        let db = Db::open_temporary().unwrap();
        let dir = temp_dir("in-progress");
        let file = dir.join("recording-2.mp4");
        std::fs::write(&file, b"still being written").unwrap();
        db.begin_recording(&file.to_string_lossy(), 1_000).unwrap();

        let report = reconcile(&db, &dir, None).unwrap();
        assert_eq!(report.imported, 0, "the file is ours and already tracked");
        assert_eq!(report.orphans_removed, 0);
        assert_eq!(db.unfinished_recordings().unwrap().len(), 1, "still in flight");
        assert!(db.list_recordings().unwrap().is_empty(), "and still not in the library");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_recordings_dir_is_not_an_error() {
        let db = Db::open_temporary().unwrap();
        let dir = std::env::temp_dir().join("ninja-recorder-reconcile-does-not-exist");
        std::fs::remove_dir_all(&dir).ok();

        let report = reconcile(&db, &dir, None).unwrap();
        assert_eq!(report, ReconcileReport::default());
    }
}
