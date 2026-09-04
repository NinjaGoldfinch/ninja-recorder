//! Disk retention: max total size AND max age, whichever bites first;
//! `pinned` recordings are exempt from deletion (but still count toward
//! usage — they still occupy disk). DEVELOPMENT.md §6 — a launch feature,
//! not optional, since 1080p60 capture (~3.5 GB/hour) fills an SSD in
//! weeks unattended. Enforced on app start and after every finalize
//! (`lib.rs`'s `setup`, `state_machine::supervisor::stop_recording`).
//!
//! Also home to the record-start free-space preflight check, since it's
//! the same "don't let capture run the disk dry" concern.

use crate::db::{Db, DbError, RecordingRow, RetentionPolicy};
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct EnforcementReport {
    pub deleted: Vec<i64>,
    pub freed_bytes: i64,
}

#[derive(Debug, thiserror::Error)]
pub enum DeleteError {
    #[error(transparent)]
    Db(#[from] DbError),
    #[error("failed to remove {path}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
}

/// Pure decision: which recording ids to delete, given the current
/// library and policy. No I/O — testable without a filesystem or DB, same
/// shape as `state_machine::machine`'s pure transition function.
///
/// Age is checked first (anything over the limit goes regardless of total
/// size), then size (oldest non-pinned recordings removed until under the
/// cap). `total` starts from *every* recording's size, pinned included,
/// since pinned files still occupy disk — only the deletion candidates
/// themselves exclude pinned rows.
pub fn select_for_deletion(
    rows: &[RecordingRow],
    policy: &RetentionPolicy,
    now_millis: i64,
) -> Vec<i64> {
    let mut candidates: Vec<&RecordingRow> = rows.iter().filter(|r| !r.pinned).collect();
    candidates.sort_by_key(|r| r.started_at); // oldest first

    let mut to_delete: Vec<i64> = Vec::new();
    let mut total: i64 = rows.iter().map(|r| r.size_bytes).sum();

    if let Some(max_age_days) = policy.max_age_days {
        let max_age_millis = max_age_days.saturating_mul(24 * 60 * 60 * 1000);
        for row in &candidates {
            if now_millis.saturating_sub(row.started_at) > max_age_millis {
                to_delete.push(row.id);
                total -= row.size_bytes;
            }
        }
    }

    if let Some(max_total_bytes) = policy.max_total_bytes {
        for row in &candidates {
            if total <= max_total_bytes {
                break;
            }
            if to_delete.contains(&row.id) {
                continue;
            }
            to_delete.push(row.id);
            total -= row.size_bytes;
        }
    }

    to_delete
}

/// Removes a recording's file and then its DB row. Markers cascade
/// (`db`'s `ON DELETE CASCADE` plus `PRAGMA foreign_keys = ON`).
///
/// A file that's already gone is not an error — the row still has to go,
/// which is exactly the orphaned-row case `reconcile` cleans up. Any other
/// io error aborts *before* the row is touched, so a user-initiated delete
/// can report "couldn't remove the file" with the library left intact
/// rather than silently dropping the entry and leaking the MP4. Retention
/// wants the opposite trade-off and opts out explicitly — see `enforce`.
pub fn delete_recording_and_file(db: &Db, row: &RecordingRow) -> Result<(), DeleteError> {
    match std::fs::remove_file(&row.path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(DeleteError::Io {
                path: row.path.clone(),
                source: e,
            })
        }
    }
    db.delete_recording(row.id).map_err(DeleteError::from)
}

/// Dry run: what `enforce` *would* do under `policy`, without touching
/// anything. Lets the settings UI warn before a tightened limit silently
/// removes VODs — the policy form is the one place in the app where a
/// careless edit destroys footage.
pub fn preview(db: &Db, policy: &RetentionPolicy) -> Result<EnforcementReport, DbError> {
    let rows = db.list_recordings()?;
    let deleted = select_for_deletion(&rows, policy, now_millis());
    let freed_bytes = deleted
        .iter()
        .filter_map(|id| rows.iter().find(|r| r.id == *id))
        .map(|r| r.size_bytes)
        .sum();
    Ok(EnforcementReport {
        deleted,
        freed_bytes,
    })
}

/// Applies `select_for_deletion` against the real DB + filesystem: removes
/// each selected recording's file (best-effort — a file already gone
/// doesn't stop its DB row from being removed too) and its DB row.
pub fn enforce(db: &Db, policy: &RetentionPolicy, now_millis: i64) -> Result<EnforcementReport, DbError> {
    let rows = db.list_recordings()?;
    let ids = select_for_deletion(&rows, policy, now_millis);

    let mut report = EnforcementReport::default();
    for id in ids {
        let Some(row) = rows.iter().find(|r| r.id == id) else {
            continue;
        };
        // Retention runs unattended, so a file it can't remove must not
        // stall the sweep: log it and drop the row anyway. `reconcile`
        // re-imports the leftover file later if it's still there.
        match delete_recording_and_file(db, row) {
            Ok(()) => {}
            Err(DeleteError::Io { path, source }) => {
                eprintln!("[retention] failed to remove {path}: {source}");
                db.delete_recording(id)?;
            }
            Err(DeleteError::Db(e)) => return Err(e),
        }
        report.deleted.push(id);
        report.freed_bytes += row.size_bytes;
    }

    Ok(report)
}

/// `enforce` against the real clock — the impure entry point every real
/// call site uses; `enforce` itself stays testable with an injected time.
pub fn enforce_now(db: &Db, policy: &RetentionPolicy) -> Result<EnforcementReport, DbError> {
    enforce(db, policy, now_millis())
}

fn now_millis() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Free space on the volume containing `dir`. `dir` itself doesn't need to
/// exist yet (the recordings folder is only created on first recording) —
/// falls back to its parent, which is created earlier at app startup.
pub fn free_space_bytes(dir: &Path) -> std::io::Result<u64> {
    let probe = if dir.exists() {
        dir
    } else {
        dir.parent().unwrap_or(dir)
    };
    fs2::available_space(probe)
}

/// Minimum free space required to start a new recording. DEVELOPMENT.md
/// §6: 1080p60@8Mbps ≈ 3.5 GB/hour, so 1 GiB is a deliberately
/// conservative floor (under 20 minutes of headroom) — retention's job is
/// to keep well above this in normal operation, this is the last-resort
/// refusal so capture doesn't run the disk dry mid-game.
pub const MIN_FREE_BYTES_TO_RECORD: u64 = 1024 * 1024 * 1024;

/// Fails open (returns `true`) on a stat error rather than block
/// recording over a free-space check we couldn't even perform.
pub fn has_room_to_record(dir: &Path) -> bool {
    match free_space_bytes(dir) {
        Ok(free) => free >= MIN_FREE_BYTES_TO_RECORD,
        Err(e) => {
            eprintln!(
                "[retention] failed to check free space for {}: {e}",
                dir.display()
            );
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: i64, started_at: i64, size_bytes: i64, pinned: bool) -> RecordingRow {
        RecordingRow {
            id,
            path: format!("/rec-{id}.mp4"),
            started_at,
            duration_s: None,
            game_id: None,
            queue: None,
            champion: None,
            role: None,
            win: None,
            kda_k: None,
            kda_d: None,
            kda_a: None,
            patch: None,
            pinned,
            size_bytes,
        }
    }

    const DAY_MS: i64 = 24 * 60 * 60 * 1000;

    #[test]
    fn no_policy_deletes_nothing() {
        let rows = vec![row(1, 0, 1_000_000_000, false)];
        let policy = RetentionPolicy {
            max_total_bytes: None,
            max_age_days: None,
        };
        assert!(select_for_deletion(&rows, &policy, 100 * DAY_MS).is_empty());
    }

    #[test]
    fn age_limit_deletes_only_older_non_pinned_rows() {
        let rows = vec![
            row(1, 0, 100, false),           // 100 days old — over the limit
            row(2, 90 * DAY_MS, 100, false), // 10 days old — under
            row(3, 0, 100, true),            // pinned, exempt despite age
        ];
        let policy = RetentionPolicy {
            max_total_bytes: None,
            max_age_days: Some(30),
        };
        let deleted = select_for_deletion(&rows, &policy, 100 * DAY_MS);
        assert_eq!(deleted, vec![1]);
    }

    #[test]
    fn size_limit_deletes_oldest_non_pinned_first() {
        let rows = vec![
            row(1, 0, 500, false),   // oldest
            row(2, 10, 500, false),
            row(3, 20, 500, false),  // newest
        ];
        let policy = RetentionPolicy {
            max_total_bytes: Some(700), // must drop below 700 total (1500 now)
            max_age_days: None,
        };
        let deleted = select_for_deletion(&rows, &policy, 0);
        // Dropping id 1 alone leaves 1000 (> 700); also drop id 2 -> 500 (<= 700).
        assert_eq!(deleted, vec![1, 2]);
    }

    #[test]
    fn size_limit_skips_pinned_even_if_it_is_oldest() {
        let rows = vec![
            row(1, 0, 500, true),  // oldest, but pinned
            row(2, 10, 500, false),
        ];
        let policy = RetentionPolicy {
            max_total_bytes: Some(700),
            max_age_days: None,
        };
        // Total is 1000, over budget, but only id 2 can be deleted — even
        // deleting it only gets to 500, still under 700, so it's enough.
        let deleted = select_for_deletion(&rows, &policy, 0);
        assert_eq!(deleted, vec![2]);
    }

    #[test]
    fn size_limit_cannot_go_below_pinned_total() {
        let rows = vec![row(1, 0, 5_000, true)];
        let policy = RetentionPolicy {
            max_total_bytes: Some(100),
            max_age_days: None,
        };
        // Nothing non-pinned exists to delete, so the policy simply can't
        // be satisfied — not an error, just no candidates.
        assert!(select_for_deletion(&rows, &policy, 0).is_empty());
    }

    #[test]
    fn age_and_size_combine_without_double_deleting() {
        let rows = vec![
            row(1, 0, 500, false),  // over age limit
            row(2, 10, 500, false), // under age, but still needed for size
        ];
        let policy = RetentionPolicy {
            max_total_bytes: Some(100),
            max_age_days: Some(30),
        };
        let deleted = select_for_deletion(&rows, &policy, 100 * DAY_MS);
        assert_eq!(deleted, vec![1, 2]);
    }

    #[test]
    fn enforce_removes_files_and_rows_and_reports_freed_bytes() {
        let db = Db::open_in_memory().unwrap();
        let dir = std::env::temp_dir().join(format!(
            "ninja-recorder-retention-test-{}-{}",
            std::process::id(),
            line!()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("old.mp4");
        std::fs::write(&path, b"data").unwrap();

        db.insert_recording(&crate::db::NewRecording {
            path: path.display().to_string(),
            started_at: 0,
            size_bytes: 4,
            ..Default::default()
        })
        .unwrap();

        let policy = RetentionPolicy {
            max_total_bytes: None,
            max_age_days: Some(1),
        };
        let report = enforce(&db, &policy, 30 * DAY_MS).unwrap();

        assert_eq!(report.deleted.len(), 1);
        assert_eq!(report.freed_bytes, 4);
        assert!(!path.exists());
        assert!(db.list_recordings().unwrap().is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }
}
