//! Optional fixture recording. When `NINJA_RECORDER_RECORD_FIXTURES` is
//! set, every LCU response this client sees is written to
//! `fixtures/lcu/<endpoint>.json`, so real response shapes can be replayed
//! in tests without a live client running. Off by default — never runs in
//! a normal session. DEVELOPMENT.md §3.3.

use std::path::PathBuf;

pub fn enabled() -> bool {
    std::env::var_os("NINJA_RECORDER_RECORD_FIXTURES").is_some()
}

/// Records `raw_json` (pretty-printed if valid JSON) under a filename
/// derived from `name` (typically the request path). No-op unless fixture
/// recording is enabled; failures are swallowed — this must never break a
/// real request.
pub fn record(name: &str, raw_json: &str) {
    if !enabled() {
        return;
    }
    let dir = fixtures_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let path = dir.join(format!("{}.json", sanitize(name)));
    let pretty = serde_json::from_str::<serde_json::Value>(raw_json)
        .ok()
        .and_then(|v| serde_json::to_string_pretty(&v).ok())
        .unwrap_or_else(|| raw_json.to_string());
    let _ = std::fs::write(path, pretty);
}

fn sanitize(name: &str) -> String {
    let trimmed = name.trim_start_matches('/');
    let cleaned: String = trimmed
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    if cleaned.is_empty() {
        "root".to_string()
    } else {
        cleaned
    }
}

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("fixtures")
        .join("lcu")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_turns_path_into_safe_filename() {
        assert_eq!(
            sanitize("/lol-gameflow/v1/gameflow-phase"),
            "lol-gameflow_v1_gameflow-phase"
        );
        assert_eq!(sanitize("/"), "root");
    }
}
