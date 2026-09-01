//! Optional fixture recording, shared by every API client (LCU, Live
//! Client Data). When `NINJA_RECORDER_RECORD_FIXTURES` is set, every
//! response is written to `<base>/<group>/<endpoint>.json`, so real
//! response shapes can be replayed in tests without a live client running.
//! Off by default — never runs in a normal session. DEVELOPMENT.md §3.3.

use std::path::PathBuf;
use std::sync::OnceLock;

static BASE_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Sets the directory fixtures are written under. Call once at startup
/// with a runtime-resolved, writable location — the app data dir, in
/// practice — before any watcher/poller starts. Without this, the
/// fallback in `fixtures_dir` (a path relative to where the source tree
/// was compiled) only exists on the machine that built the binary, which
/// makes fixture recording silently do nothing on an installed copy: the
/// path doesn't exist, `create_dir_all` fails, and `record`'s errors are
/// swallowed by design. Safe to call more than once; only the first call
/// takes effect.
pub fn set_base_dir(dir: PathBuf) {
    let _ = BASE_DIR.set(dir);
}

pub fn enabled() -> bool {
    std::env::var_os("NINJA_RECORDER_RECORD_FIXTURES").is_some()
}

/// Records `raw_json` (pretty-printed if valid JSON) under
/// `fixtures/<group>/<name>.json`. `group` separates namespaces that could
/// otherwise collide (LCU paths vs. Live Client Data paths). No-op unless
/// fixture recording is enabled; failures are swallowed — this must never
/// break a real request.
pub fn record(group: &str, name: &str, raw_json: &str) {
    if !enabled() {
        return;
    }
    let dir = fixtures_dir(group);
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

fn fixtures_dir(group: &str) -> PathBuf {
    let base = BASE_DIR.get_or_init(|| {
        // Dev-from-source fallback: writes into the repo's own fixtures/
        // dir, matching where `cargo tauri dev` reads sample fixtures
        // from. Only reachable if `set_base_dir` is never called — the
        // real app always calls it during startup before this can run.
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("fixtures")
    });
    base.join(group)
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
