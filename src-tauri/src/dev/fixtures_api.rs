//! Browsing, editing, and capturing API fixtures.
//!
//! `fixtures.rs` writes every LCU / Live Client Data response to disk when
//! capture is on, but nothing has ever read them back — DEVELOPMENT.md
//! §3.3 asks for a replay mode and none existed. These commands make the
//! captured files listable and loadable, which is what lets the Simulate
//! panel feed a real recorded payload back through the pipeline.

use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Serialize)]
pub struct FixtureEntry {
    /// `lcu`, `live-client`, or whatever subdirectory it was found under.
    pub group: String,
    pub name: String,
    pub path: String,
    pub bytes: u64,
    pub modified_millis: Option<i64>,
    /// Which root it came from: the app data dir (captured at runtime) or
    /// the repo (checked in). Both are useful and they are not the same
    /// set, so the portal shows the origin rather than merging them.
    pub source: &'static str,
}

#[derive(Serialize)]
pub struct FixturesState {
    pub recording_enabled: bool,
    pub capture_dir: Option<String>,
    pub repo_dir: Option<String>,
    pub entries: Vec<FixtureEntry>,
}

#[tauri::command]
pub fn dev_fixtures_state() -> Result<FixturesState, String> {
    let capture_dir = crate::fixtures::base_dir();
    let repo_dir = super::info::repo_fixtures_dir();

    let mut entries = Vec::new();
    if let Some(dir) = &capture_dir {
        collect(dir, "captured", &mut entries);
    }
    if let Some(dir) = &repo_dir {
        collect(dir, "repo", &mut entries);
    }
    entries.sort_by(|a, b| (a.source, &a.group, &a.name).cmp(&(b.source, &b.group, &b.name)));

    Ok(FixturesState {
        recording_enabled: crate::fixtures::enabled(),
        capture_dir: capture_dir.map(|d| d.display().to_string()),
        repo_dir: repo_dir.map(|d| d.display().to_string()),
        entries,
    })
}

/// Walks one fixture root, one level deep plus the root itself. Fixtures
/// are written as `<base>/<group>/<endpoint>.json`, so there is nothing
/// deeper to find and a full recursive walk would only risk wandering into
/// an unrelated directory a user pointed the base dir at.
fn collect(root: &Path, source: &'static str, out: &mut Vec<FixtureEntry>) {
    let mut push = |path: PathBuf, group: &str| {
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            return;
        }
        let meta = match std::fs::metadata(&path) {
            Ok(m) => m,
            Err(_) => return,
        };
        out.push(FixtureEntry {
            group: group.to_string(),
            name: path
                .file_stem()
                .and_then(|n| n.to_str())
                .unwrap_or("?")
                .to_string(),
            path: path.display().to_string(),
            bytes: meta.len(),
            modified_millis: meta.modified().ok().and_then(|t| {
                t.duration_since(std::time::UNIX_EPOCH)
                    .ok()
                    .map(|d| d.as_millis() as i64)
            }),
            source,
        });
    };

    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let group = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("?")
                .to_string();
            if let Ok(children) = std::fs::read_dir(&path) {
                for child in children.flatten() {
                    push(child.path(), &group);
                }
            }
        } else {
            push(path, "");
        }
    }
}

/// Reads one fixture. Confined to the two known roots — the path comes
/// back from `dev_fixtures_state`, but it arrives over IPC as a plain
/// string, and a dev command is no reason to accept `../../etc/passwd`.
#[tauri::command]
pub fn dev_fixture_read(path: String) -> Result<String, String> {
    let path = checked_path(&path)?;
    std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Saves a payload as a fixture under the capture directory, so a
/// hand-edited snapshot can be replayed later.
#[tauri::command]
pub fn dev_fixture_write(group: String, name: String, contents: String) -> Result<String, String> {
    // Fail before writing rather than leaving unparseable JSON behind for
    // the injector to choke on later.
    let value: serde_json::Value =
        serde_json::from_str(&contents).map_err(|e| format!("not valid JSON: {e}"))?;

    let base = crate::fixtures::base_dir()
        .ok_or_else(|| "fixtures directory not initialized".to_string())?;
    let dir = base.join(sanitize(&group));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    let path = dir.join(format!("{}.json", sanitize(&name)));
    let pretty = serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?;
    std::fs::write(&path, pretty).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path.display().to_string())
}

/// Turns fixture capture on or off for the running process. Previously
/// only settable by launching with `NINJA_RECORDER_RECORD_FIXTURES` set,
/// which meant deciding to capture a game before starting the app.
#[tauri::command]
pub fn dev_set_fixture_recording(enabled: bool) -> bool {
    crate::fixtures::set_enabled(enabled);
    crate::fixtures::enabled()
}

/// Mirrors `fixtures::sanitize` — same rule, since these files sit in the
/// same tree and are listed by the same walker.
fn sanitize(name: &str) -> String {
    let cleaned: String = name
        .trim_start_matches('/')
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "root".to_string()
    } else {
        cleaned
    }
}

fn checked_path(path: &str) -> Result<PathBuf, String> {
    let candidate = Path::new(path)
        .canonicalize()
        .map_err(|e| format!("{path}: {e}"))?;

    let roots = [crate::fixtures::base_dir(), super::info::repo_fixtures_dir()];
    let allowed = roots
        .iter()
        .flatten()
        .filter_map(|r| r.canonicalize().ok())
        .any(|root| candidate.starts_with(root));

    if allowed {
        Ok(candidate)
    } else {
        Err(format!(
            "{path} is outside the fixture directories; only files listed by dev_fixtures_state can be read"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::sanitize;

    #[test]
    fn sanitize_matches_the_fixtures_module_rule() {
        assert_eq!(sanitize("/liveclientdata/allgamedata"), "liveclientdata_allgamedata");
        assert_eq!(sanitize("live-client"), "live-client");
        assert_eq!(sanitize("../../etc"), "______etc");
        assert_eq!(sanitize(""), "root");
    }
}
