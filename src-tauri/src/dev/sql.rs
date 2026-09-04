//! Schema-aware table browser, row editor, and raw SQL console.
//!
//! Everything here bypasses the typed `db` API — including the `path`
//! upsert rule and the `NewRecording`/`NewMarker` shapes — which is the
//! whole point (reproducing a specific bad row is otherwise impossible)
//! and also exactly why this module is behind the `devtools` feature.

use crate::{dev, AppState};
use rusqlite::types::{Value as SqlValue, ValueRef};
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

/// Tables the browser and row editor will touch. `sqlite_sequence` and
/// the migration bookkeeping table are deliberately absent — they are
/// reachable from the SQL console for anyone who really wants them, but
/// they should not show up as ordinary editable data.
const BROWSABLE_TABLES: &[&str] = &["recordings", "markers", "samples", "settings"];

#[derive(Serialize)]
pub struct Column {
    pub name: String,
    pub decl_type: String,
    pub not_null: bool,
    pub default_value: Option<String>,
    pub pk: bool,
}

#[derive(Serialize)]
pub struct TableSchema {
    pub name: String,
    pub columns: Vec<Column>,
    pub row_count: i64,
}

/// Live schema, read from `PRAGMA table_info`. The portal generates its
/// insert and edit forms from this rather than hardcoding today's columns,
/// so a new migration shows up in the UI without a frontend change.
#[tauri::command]
pub fn dev_schema(state: tauri::State<AppState>) -> Result<Vec<TableSchema>, String> {
    let conn = state.db.conn();
    let mut out = Vec::new();

    for table in BROWSABLE_TABLES {
        let mut stmt = conn
            .prepare(&format!("PRAGMA table_info({table})"))
            .map_err(|e| e.to_string())?;
        let columns = stmt
            .query_map([], |row| {
                Ok(Column {
                    name: row.get(1)?,
                    decl_type: row.get(2)?,
                    not_null: row.get::<_, i64>(3)? != 0,
                    default_value: row.get(4)?,
                    pk: row.get::<_, i64>(5)? != 0,
                })
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;

        let row_count: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .map_err(|e| e.to_string())?;

        out.push(TableSchema {
            name: (*table).to_string(),
            columns,
            row_count,
        });
    }
    Ok(out)
}

#[derive(Serialize)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Json>>,
    pub rows_affected: usize,
    pub elapsed_ms: f64,
    /// True when the statement returned a result set rather than a row
    /// count — the portal renders a grid for one and a summary for the
    /// other.
    pub returned_rows: bool,
}

/// A paged read of one table. Separate from `dev_sql_query` so the common
/// case can't be a typo away from a `DELETE`.
#[tauri::command]
pub fn dev_table_page(
    state: tauri::State<AppState>,
    table: String,
    limit: Option<i64>,
    offset: Option<i64>,
    order_by: Option<String>,
) -> Result<QueryResult, String> {
    let table = checked_table(&table)?;
    let limit = limit.unwrap_or(100).clamp(1, 1000);
    let offset = offset.unwrap_or(0).max(0);

    // `order_by` is validated against the table's real columns rather than
    // interpolated blind — it can't be bound as a parameter.
    let order = match order_by {
        Some(spec) => {
            let (col, dir) = spec.split_once(' ').unwrap_or((spec.as_str(), "ASC"));
            let dir = match dir.trim().to_uppercase().as_str() {
                "DESC" => "DESC",
                _ => "ASC",
            };
            let col = checked_column(&state, table, col.trim())?;
            format!("ORDER BY \"{col}\" {dir}")
        }
        None => String::new(),
    };

    run_sql(
        &state,
        &format!("SELECT * FROM {table} {order} LIMIT {limit} OFFSET {offset}"),
    )
}

/// Arbitrary SQL against the live library DB. The portal shows the
/// resolved DB path in its header so there is never doubt about which file
/// this lands in.
#[tauri::command]
pub fn dev_sql_query(
    state: tauri::State<AppState>,
    app: tauri::AppHandle,
    sql: String,
) -> Result<QueryResult, String> {
    let result = run_sql(&state, &sql)?;
    if !result.returned_rows {
        dev::notify_library_changed(&app);
    }
    Ok(result)
}

/// Returns true for statements that produce a result set. `prepare` +
/// `column_count` would be more robust than sniffing the text, but a
/// prepared `DELETE` reports zero columns either way, and this keeps the
/// classification visible and unit-testable.
pub(crate) fn returns_rows(sql: &str) -> bool {
    let head = sql
        .trim_start()
        .lines()
        .find(|l| !l.trim_start().starts_with("--") && !l.trim().is_empty())
        .unwrap_or("")
        .trim_start()
        .to_ascii_uppercase();
    ["SELECT", "PRAGMA", "WITH", "EXPLAIN", "VALUES"]
        .iter()
        .any(|kw| head.starts_with(kw))
}

fn run_sql(state: &tauri::State<AppState>, sql: &str) -> Result<QueryResult, String> {
    let conn = state.db.conn();
    let started = std::time::Instant::now();

    if returns_rows(sql) {
        let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
        let columns: Vec<String> = stmt.column_names().iter().map(|c| c.to_string()).collect();
        let width = columns.len();

        let rows = stmt
            .query_map([], |row| {
                (0..width)
                    .map(|i| Ok(value_ref_to_json(row.get_ref(i)?)))
                    .collect::<Result<Vec<Json>, rusqlite::Error>>()
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;

        Ok(QueryResult {
            rows_affected: rows.len(),
            columns,
            rows,
            elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
            returned_rows: true,
        })
    } else {
        // `execute_batch` so multi-statement pastes work, then a separate
        // `changes()` read — `execute` refuses anything with a trailing
        // second statement.
        conn.execute_batch(sql).map_err(|e| e.to_string())?;
        Ok(QueryResult {
            columns: Vec::new(),
            rows: Vec::new(),
            rows_affected: conn.changes() as usize,
            elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
            returned_rows: false,
        })
    }
}

#[derive(Deserialize)]
pub struct RowValues(pub std::collections::BTreeMap<String, Json>);

#[tauri::command]
pub fn dev_insert_row(
    state: tauri::State<AppState>,
    app: tauri::AppHandle,
    table: String,
    values: RowValues,
) -> Result<i64, String> {
    let table = checked_table(&table)?;
    if values.0.is_empty() {
        return Err("no values given".to_string());
    }

    let mut names = Vec::new();
    let mut binds: Vec<SqlValue> = Vec::new();
    for (name, value) in &values.0 {
        names.push(format!("\"{}\"", checked_column(&state, table, name)?));
        binds.push(json_to_sql(value)?);
    }
    let placeholders = vec!["?"; names.len()].join(", ");

    let id = {
        let conn = state.db.conn();
        conn.execute(
            &format!(
                "INSERT INTO {table} ({}) VALUES ({placeholders})",
                names.join(", ")
            ),
            rusqlite::params_from_iter(binds.iter()),
        )
        .map_err(|e| e.to_string())?;
        conn.last_insert_rowid()
    };
    dev::notify_library_changed(&app);
    Ok(id)
}

#[tauri::command]
pub fn dev_update_row(
    state: tauri::State<AppState>,
    app: tauri::AppHandle,
    table: String,
    id: i64,
    values: RowValues,
) -> Result<usize, String> {
    let table = checked_table(&table)?;
    if values.0.is_empty() {
        return Err("no values given".to_string());
    }

    let mut assignments = Vec::new();
    let mut binds: Vec<SqlValue> = Vec::new();
    for (name, value) in &values.0 {
        assignments.push(format!("\"{}\" = ?", checked_column(&state, table, name)?));
        binds.push(json_to_sql(value)?);
    }
    binds.push(SqlValue::Integer(id));

    let changed = {
        let conn = state.db.conn();
        conn.execute(
            &format!(
                "UPDATE {table} SET {} WHERE {} = ?",
                assignments.join(", "),
                primary_key(table)
            ),
            rusqlite::params_from_iter(binds.iter()),
        )
        .map_err(|e| e.to_string())?
    };
    dev::notify_library_changed(&app);
    Ok(changed)
}

#[tauri::command]
pub fn dev_delete_row(
    state: tauri::State<AppState>,
    app: tauri::AppHandle,
    table: String,
    id: i64,
    // `delete_file` only means anything for `recordings`. Without it the
    // next `reconcile` sees an untracked file and imports the row straight
    // back, which reads as the delete having silently failed.
    delete_file: Option<bool>,
) -> Result<usize, String> {
    let table = checked_table(&table)?;

    if table == "recordings" && delete_file.unwrap_or(false) {
        let conn = state.db.conn();
        let path: Option<String> = conn
            .query_row("SELECT path FROM recordings WHERE id = ?", [id], |r| r.get(0))
            .ok();
        drop(conn);
        if let Some(path) = path {
            let _ = std::fs::remove_file(path);
        }
    }

    let changed = {
        let conn = state.db.conn();
        conn.execute(
            &format!("DELETE FROM {table} WHERE {} = ?", primary_key(table)),
            [id],
        )
        .map_err(|e| e.to_string())?
    };
    dev::notify_library_changed(&app);
    Ok(changed)
}

#[derive(Serialize)]
pub struct ResetReport {
    pub rows_deleted: usize,
    pub files_deleted: usize,
}

/// Returns the app to first-launch state: every row gone, autoincrement
/// counters reset, the single `settings` row re-seeded with its migration
/// defaults (50 GiB / 30 days).
///
/// Deliberately empties the tables rather than deleting and re-migrating
/// the DB file — `AppState` holds a live `Arc<Db>` with an open
/// connection, and swapping that out mid-session is a real hazard for an
/// identical observable result.
#[tauri::command]
pub fn dev_reset_db(
    state: tauri::State<AppState>,
    app: tauri::AppHandle,
    also_clear_files: bool,
) -> Result<ResetReport, String> {
    let mut files_deleted = 0usize;
    if also_clear_files {
        if let Ok(entries) = std::fs::read_dir(&state.recordings_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let is_video = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| matches!(e.to_lowercase().as_str(), "mp4" | "mkv"));
                if is_video && std::fs::remove_file(&path).is_ok() {
                    files_deleted += 1;
                }
            }
        }
    }

    let rows_deleted = {
        let mut conn = state.db.conn();
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let mut deleted = 0usize;
        // markers/samples cascade from recordings, but deleting them
        // explicitly keeps this correct if a future migration drops the
        // foreign key.
        for table in ["markers", "samples", "recordings", "settings"] {
            deleted += tx
                .execute(&format!("DELETE FROM {table}"), [])
                .map_err(|e| e.to_string())?;
        }
        tx.execute(
            "DELETE FROM sqlite_sequence WHERE name IN ('recordings','markers','samples')",
            [],
        )
        .map_err(|e| e.to_string())?;
        // Re-seed the settings row exactly as migration 2 does, defaults
        // included — an app with no settings row reads as a DB error, not
        // as "unlimited".
        tx.execute(
            "INSERT INTO settings (id, max_total_bytes, max_age_days) VALUES (1, 53687091200, 30)",
            [],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        deleted
    };

    dev::notify_library_changed(&app);
    Ok(ResetReport {
        rows_deleted,
        files_deleted,
    })
}

fn checked_table(table: &str) -> Result<&'static str, String> {
    BROWSABLE_TABLES
        .iter()
        .find(|t| **t == table)
        .copied()
        .ok_or_else(|| format!("unknown table: {table}"))
}

/// Column names can't be bound as parameters, so every one that reaches a
/// format string is first matched against the table's real schema.
fn checked_column(
    state: &tauri::State<AppState>,
    table: &str,
    column: &str,
) -> Result<String, String> {
    let conn = state.db.conn();
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(|e| e.to_string())?;
    let found = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|name| name == column);
    found.ok_or_else(|| format!("unknown column {column} on {table}"))
}

fn primary_key(table: &str) -> &'static str {
    // Every table here keys on `id`, `settings` included (it's a
    // single-row table with `CHECK (id = 1)`).
    let _ = table;
    "id"
}

fn value_ref_to_json(value: ValueRef<'_>) -> Json {
    match value {
        ValueRef::Null => Json::Null,
        ValueRef::Integer(i) => Json::from(i),
        ValueRef::Real(f) => serde_json::Number::from_f64(f).map(Json::Number).unwrap_or(Json::Null),
        ValueRef::Text(t) => Json::String(String::from_utf8_lossy(t).into_owned()),
        ValueRef::Blob(b) => Json::String(format!("<blob {} bytes>", b.len())),
    }
}

fn json_to_sql(value: &Json) -> Result<SqlValue, String> {
    Ok(match value {
        Json::Null => SqlValue::Null,
        Json::Bool(b) => SqlValue::Integer(i64::from(*b)),
        Json::Number(n) => {
            if let Some(i) = n.as_i64() {
                SqlValue::Integer(i)
            } else if let Some(f) = n.as_f64() {
                SqlValue::Real(f)
            } else {
                return Err(format!("unrepresentable number: {n}"));
            }
        }
        Json::String(s) => SqlValue::Text(s.clone()),
        // Objects and arrays land in TEXT columns as JSON — which is
        // exactly what `markers.payload_json` holds.
        other => SqlValue::Text(other.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::returns_rows;

    #[test]
    fn classifies_result_set_statements() {
        assert!(returns_rows("SELECT * FROM recordings"));
        assert!(returns_rows("  select 1"));
        assert!(returns_rows("PRAGMA table_info(recordings)"));
        assert!(returns_rows("WITH x AS (SELECT 1) SELECT * FROM x"));
        assert!(returns_rows("EXPLAIN QUERY PLAN SELECT 1"));
    }

    #[test]
    fn classifies_mutations_as_not_returning_rows() {
        assert!(!returns_rows("DELETE FROM markers"));
        assert!(!returns_rows("UPDATE recordings SET pinned = 1"));
        assert!(!returns_rows("INSERT INTO settings (id) VALUES (1)"));
        assert!(!returns_rows("  vacuum"));
    }

    #[test]
    fn leading_comments_and_blank_lines_do_not_hide_the_verb() {
        assert!(returns_rows("-- how many?\nSELECT COUNT(*) FROM samples"));
        assert!(!returns_rows("\n-- danger\nDELETE FROM recordings"));
    }
}
