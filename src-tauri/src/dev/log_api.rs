//! Reading the backend log back (#72).
//!
//! `log.rs` writes a file in release builds because a shipped app has no
//! console (DEVELOPMENT.md §13). These commands are the other half: the
//! only thing that reads it. They stay behind `devtools` so the shipped
//! surface remains one file and no UI.
//!
//! **Thin by design.** Parsing a line and deciding whether it survives the
//! filters are pure functions in `log.rs`, next to the formatter that
//! defines the format — a reader that re-describes it somewhere else
//! drifts from it the first time either changes. Everything here is file
//! I/O and shaping for the webview.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// Hard ceiling on lines returned in one call.
///
/// The file is capped at 5 MiB, which is far too much to hand a webview in
/// one string — and a panel rendering a hundred thousand rows is unusable
/// regardless. The portal asks for a window and says so when there is more.
const MAX_LINES: usize = 2_000;

#[derive(Serialize)]
pub struct LogFileInfo {
    pub name: String,
    pub bytes: u64,
    pub modified_millis: Option<i64>,
    /// The file currently being appended to, as against a rotated one.
    pub active: bool,
    pub exists: bool,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LogQuery {
    /// Which file, by name. `None` means the active one.
    pub file: Option<String>,
    /// Empty means every level, not none.
    #[serde(default)]
    pub levels: Vec<String>,
    /// Tags to **hide**, not to show. Exclusion rather than inclusion
    /// because the panel has to hide the noisy streams before it has read
    /// the file and learned which tags exist — see `log::line_matches`.
    #[serde(default, rename = "hideTags")]
    pub hide_tags: Vec<String>,
    #[serde(default)]
    pub search: String,
    pub limit: Option<usize>,
}

#[derive(Serialize)]
pub struct LogPage {
    pub file: String,
    pub dir: Option<String>,
    /// Lines in the file, before filtering.
    pub total: usize,
    /// Lines that survived the filters, which may exceed those returned.
    pub matched: usize,
    /// True when `matched` was larger than the window returned.
    pub truncated: bool,
    /// Oldest first, so the newest is at the bottom the way a log reads.
    pub lines: Vec<crate::log::ParsedLine>,
    /// Every tag present in the file, so the panel can offer real filter
    /// buttons rather than a hardcoded list that goes stale when a new tag
    /// is added.
    pub tags_present: Vec<String>,
}

/// The log files that exist, newest first, including rotated ones.
///
/// Reports the ones that are *missing* too: "there is no log file" and
/// "the log file is empty" are different answers, and a panel that shows
/// nothing for both is one nobody can trust.
#[tauri::command]
pub fn dev_log_files() -> Result<Vec<LogFileInfo>, String> {
    let dir = crate::log::dir().ok_or_else(|| "logging is not initialized".to_string())?;
    Ok(log_files_in(&dir))
}

/// Our own files first, in rotation order, then anything else the
/// directory holds — on Windows that is `libobs.log`, which the capture
/// worker writes and which nothing else would list (#69).
fn log_files_in(dir: &std::path::Path) -> Vec<LogFileInfo> {
    let ours = crate::log::file_names();
    let mut files: Vec<LogFileInfo> = ours
        .iter()
        .enumerate()
        .map(|(i, name)| describe(dir, name.clone(), i == 0))
        .collect();

    // Sorted, because `read_dir` order is whatever the filesystem says and
    // a list that reshuffles between reloads is one nobody can use.
    let mut extra: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.ends_with(".log") && !ours.contains(name))
        .collect();
    extra.sort();
    files.extend(extra.into_iter().map(|name| describe(dir, name, false)));
    files
}

fn describe(dir: &std::path::Path, name: String, active: bool) -> LogFileInfo {
    let meta = std::fs::metadata(dir.join(&name)).ok();
    LogFileInfo {
        exists: meta.is_some(),
        bytes: meta.as_ref().map(|m| m.len()).unwrap_or(0),
        modified_millis: meta
            .as_ref()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64),
        active,
        name,
    }
}

/// One filtered window of a log file.
///
/// Returns the **newest** matching lines, because that is what anyone
/// opening a log wants; `truncated` says whether older matches were left
/// behind rather than silently dropping them.
#[tauri::command]
pub fn dev_read_log(query: LogQuery) -> Result<LogPage, String> {
    let dir = crate::log::dir().ok_or_else(|| "logging is not initialized".to_string())?;

    let name = match query.file {
        Some(requested) => {
            // Only a file this listing already offered. The name reaches
            // us from the webview, and joining an arbitrary string onto a
            // directory is how a log viewer turns into a file reader —
            // matching against the enumeration rules out both traversal
            // and anything outside this directory, without a separate
            // sanitizer to get subtly wrong.
            if !log_files_in(&dir).iter().any(|f| f.name == requested) {
                return Err(format!("not a log file: {requested}"));
            }
            requested
        }
        None => crate::log::file_names()[0].clone(),
    };

    let path = dir.join(&name);
    let body = match std::fs::read_to_string(&path) {
        Ok(body) => body,
        // A rotated file that does not exist yet is an empty page, not an
        // error — the portal lists all three from the start.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("could not read {}: {e}", path.display())),
    };

    // `max(1)` so a caller passing 0 gets one line rather than a page that
    // reports matches and shows none.
    let limit = query.limit.unwrap_or(500).clamp(1, MAX_LINES);
    let mut total = 0usize;
    let mut matched = 0usize;
    let mut tags_present: Vec<String> = Vec::new();
    // A deque, not a Vec: this keeps the newest `limit` lines by dropping
    // from the front, and `Vec::remove(0)` shifts every remaining element
    // each time — O(n) per line over a file with a hundred thousand of
    // them.
    let mut kept: VecDeque<crate::log::ParsedLine> = VecDeque::with_capacity(limit);

    for line in body.lines() {
        if line.is_empty() {
            continue;
        }
        total += 1;
        let parsed = crate::log::parse_line(line);
        if !parsed.tag.is_empty() && !tags_present.contains(&parsed.tag) {
            tags_present.push(parsed.tag.clone());
        }
        if !crate::log::line_matches(&parsed, &query.levels, &query.hide_tags, &query.search) {
            continue;
        }
        matched += 1;
        kept.push_back(parsed);
        // Keep only the tail, so peak memory is the window rather than the
        // whole file — which is the point of a window.
        if kept.len() > limit {
            kept.pop_front();
        }
    }

    tags_present.sort();

    Ok(LogPage {
        file: name,
        dir: Some(dir.display().to_string()),
        total,
        truncated: matched > kept.len(),
        matched,
        lines: kept.into(),
        tags_present,
    })
}
