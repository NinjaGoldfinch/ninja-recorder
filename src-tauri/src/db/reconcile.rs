//! Folder-scan reconciliation: a DB row whose file is gone gets removed;
//! a video file with no DB row gets imported as an "unknown recording".
//! DEVELOPMENT.md §4 — "the library must survive users touching the
//! folder." Run once at app startup and available on demand (dev panel /
//! future UI rescan button).

use super::{Db, DbError, NewRecording};
use serde::Serialize;
use std::path::Path;

const VIDEO_EXTENSIONS: &[&str] = &["mp4", "mkv"];

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ReconcileReport {
    pub orphans_removed: usize,
    pub imported: usize,
}

pub fn reconcile(db: &Db, recordings_dir: &Path) -> Result<ReconcileReport, DbError> {
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
            db.insert_recording(&NewRecording {
                path: path_str,
                started_at: file_modified_millis(&metadata),
                size_bytes: metadata.len() as i64,
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
        let db = Db::open_in_memory().unwrap();
        let dir = temp_dir("orphan");
        db.insert_recording(&NewRecording {
            path: dir.join("gone.mp4").to_string_lossy().to_string(),
            started_at: 1,
            ..Default::default()
        })
        .unwrap();

        let report = reconcile(&db, &dir).unwrap();
        assert_eq!(report.orphans_removed, 1);
        assert_eq!(report.imported, 0);
        assert!(db.list_recordings().unwrap().is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn imports_untracked_video_file() {
        let db = Db::open_in_memory().unwrap();
        let dir = temp_dir("import");
        std::fs::write(dir.join("untracked.mp4"), b"fake video bytes").unwrap();

        let report = reconcile(&db, &dir).unwrap();
        assert_eq!(report.orphans_removed, 0);
        assert_eq!(report.imported, 1);

        let rows = db.list_recordings().unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].path.ends_with("untracked.mp4"));
        assert!(rows[0].champion.is_none(), "imported rows carry no match metadata");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn ignores_non_video_files() {
        let db = Db::open_in_memory().unwrap();
        let dir = temp_dir("ignore");
        std::fs::write(dir.join("notes.txt"), b"not a video").unwrap();

        let report = reconcile(&db, &dir).unwrap();
        assert_eq!(report.imported, 0);
        assert!(db.list_recordings().unwrap().is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn tracked_file_is_left_alone() {
        let db = Db::open_in_memory().unwrap();
        let dir = temp_dir("tracked");
        let file_path = dir.join("known.mp4");
        std::fs::write(&file_path, b"fake video bytes").unwrap();
        db.insert_recording(&NewRecording {
            path: file_path.to_string_lossy().to_string(),
            started_at: 1,
            champion: Some("Ahri".into()),
            ..Default::default()
        })
        .unwrap();

        let report = reconcile(&db, &dir).unwrap();
        assert_eq!(report, ReconcileReport::default());

        let rows = db.list_recordings().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].champion, Some("Ahri".to_string()));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_recordings_dir_is_not_an_error() {
        let db = Db::open_in_memory().unwrap();
        let dir = std::env::temp_dir().join("ninja-recorder-reconcile-does-not-exist");
        std::fs::remove_dir_all(&dir).ok();

        let report = reconcile(&db, &dir).unwrap();
        assert_eq!(report, ReconcileReport::default());
    }
}
