//! Dry-run retention preview.
//!
//! `set_retention_policy` saves *and* immediately enforces, deleting files
//! as a side effect with nothing shown first. `select_for_deletion` is
//! pure and already public, so previewing costs nothing — this is a thin
//! wrapper that runs the same decision against the same rows and reports
//! what enforcement *would* remove.

use crate::{db, retention, AppState};
use serde::Serialize;

#[derive(Serialize)]
pub struct PreviewRow {
    pub id: i64,
    pub path: String,
    pub started_at: i64,
    pub size_bytes: i64,
    pub pinned: bool,
    pub champion: Option<String>,
    /// Whether the file backing this row is still on disk. A row selected
    /// for deletion whose file is already gone frees no bytes, which is
    /// otherwise a confusing gap between the preview estimate and the result.
    pub file_exists: bool,
}

#[derive(Serialize)]
pub struct RetentionPreview {
    pub policy: db::RetentionPolicy,
    pub now_millis: i64,
    pub total_bytes: i64,
    pub pinned_bytes: i64,
    pub to_delete: Vec<PreviewRow>,
    pub would_free_bytes: i64,
    pub total_after_bytes: i64,
}

/// Previews enforcement of `policy` (or the saved one, when `policy` is
/// omitted) without touching the DB or filesystem. `now_millis` is
/// overridable so an age rule can be tested without waiting days for it to
/// bite — `select_for_deletion` already takes an injected clock precisely
/// so it can be driven like this.
#[tauri::command]
pub fn dev_retention_preview(
    state: tauri::State<AppState>,
    policy: Option<db::RetentionPolicy>,
    now_millis: Option<i64>,
) -> Result<RetentionPreview, String> {
    let policy = match policy {
        Some(p) => p,
        None => state.db.get_retention_policy().map_err(|e| e.to_string())?,
    };
    let now_millis = now_millis.unwrap_or_else(now);

    let rows = state.db.list_recordings().map_err(|e| e.to_string())?;
    let selected = retention::select_for_deletion(&rows, &policy, now_millis);

    let to_delete: Vec<PreviewRow> = rows
        .iter()
        .filter(|r| selected.contains(&r.id))
        .map(|r| PreviewRow {
            id: r.id,
            path: r.path.clone(),
            started_at: r.started_at,
            size_bytes: r.size_bytes,
            pinned: r.pinned,
            champion: r.champion.clone(),
            file_exists: std::path::Path::new(&r.path).exists(),
        })
        .collect();

    let total_bytes: i64 = rows.iter().map(|r| r.size_bytes).sum();
    let would_free_bytes: i64 = to_delete.iter().map(|r| r.size_bytes).sum();

    Ok(RetentionPreview {
        policy,
        now_millis,
        total_bytes,
        pinned_bytes: rows.iter().filter(|r| r.pinned).map(|r| r.size_bytes).sum(),
        would_free_bytes,
        total_after_bytes: total_bytes - would_free_bytes,
        to_delete,
    })
}

fn now() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
