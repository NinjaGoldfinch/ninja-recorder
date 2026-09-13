//! Fixture recording, shared by every API client (LCU, Live Client Data).
//! Every response is written to `<base>/<group>/<endpoint>.json`, so real
//! response shapes can be replayed in tests without a live client running.
//! DEVELOPMENT.md §3.3.
//!
//! ## On by default, temporarily
//!
//! This used to be opt-in via `NINJA_RECORDER_RECORD_FIXTURES`. It is now
//! **on unless that variable says otherwise**, because almost every shape
//! this app parses was written by hand and has never been checked against
//! a real client — and the cost of that came due in #74, where a payload
//! the parser could not read ended a recording nine minutes into a game
//! and no copy of it was kept.
//!
//! Recording every response means the payload that broke something is on
//! disk when you go looking, because `record` runs *before* the parse.
//!
//! **Revert this to opt-in for the v1.0 release.** Search for
//! `DEFAULT_ON_UNTIL_V1`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

static BASE_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Seeded from `NINJA_RECORDER_RECORD_FIXTURES` at startup, then owned by
/// this flag. Reading the env var on every call would make the setting
/// immutable for the process lifetime (and mutating the environment at
/// runtime is process-global and racy with any other reader), so the env
/// var is the *initial* value and `set_enabled` — used by the dev portal
/// to flip capture off for a single game — is the running one.
///
/// Starts `false` and is set by `init_from_env` during startup, so a test
/// binary — which never calls it — writes no files.
static ENABLED: AtomicBool = AtomicBool::new(false);

/// DEFAULT_ON_UNTIL_V1: capture is on unless explicitly disabled. See the
/// module header, and flip this back for the v1.0 release.
const DEFAULT_ENABLED: bool = true;

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

/// Reads the initial enabled state from the environment. Call once at
/// startup, alongside `set_base_dir`.
///
/// The variable is now an *override*, not a switch-on: unset means the
/// `DEFAULT_ENABLED` above, and only an explicitly falsey value turns
/// capture off. That inversion is deliberate and temporary — see the
/// module header.
pub fn init_from_env() {
    let enabled = match std::env::var("NINJA_RECORDER_RECORD_FIXTURES") {
        Ok(value) => is_truthy(&value),
        // Unset, or not valid UTF-8 — neither is somebody asking for it off.
        Err(_) => DEFAULT_ENABLED,
    };
    ENABLED.store(enabled, Ordering::Relaxed);
}

/// How the env var reads. Anything explicitly falsey turns capture off;
/// anything else — including an empty value, which is how a shell spells
/// "I set this but did not think about it" — leaves the default alone.
fn is_truthy(value: &str) -> bool {
    match value.trim().to_ascii_lowercase().as_str() {
        "0" | "false" | "off" | "no" => false,
        "" => DEFAULT_ENABLED,
        _ => true,
    }
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Turns capture on or off at runtime (dev portal's Fixtures panel).
#[cfg(feature = "devtools")]
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

/// Where fixtures are currently being written, for the dev portal's
/// listing. `None` before `set_base_dir` has run.
#[cfg(feature = "devtools")]
pub fn base_dir() -> Option<PathBuf> {
    BASE_DIR.get().cloned()
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

    /// The static starts `false` and only `init_from_env` turns it on, so a
    /// test binary — which never calls that — writes no fixture files
    /// however the default is set.
    #[test]
    fn the_flag_starts_off_and_follows_set_enabled() {
        // The static is process-wide, so restore whatever it was.
        let before = enabled();
        ENABLED.store(false, Ordering::Relaxed);
        assert!(!enabled());
        ENABLED.store(true, Ordering::Relaxed);
        assert!(enabled());
        ENABLED.store(before, Ordering::Relaxed);
    }

    /// The variable is an override, not a switch-on: capture is on unless
    /// something explicitly says otherwise (DEFAULT_ON_UNTIL_V1).
    #[test]
    fn only_an_explicitly_falsey_value_turns_capture_off() {
        assert!(!is_truthy("0"));
        assert!(!is_truthy("false"));
        assert!(!is_truthy(" OFF "));
        assert!(!is_truthy("no"));
    }

    #[test]
    fn anything_else_leaves_capture_on() {
        assert!(is_truthy("1"));
        assert!(is_truthy("true"));
        assert!(is_truthy("yes"));
        // How a shell spells "I set this but did not think about it".
        assert_eq!(is_truthy(""), DEFAULT_ENABLED);
    }

    #[test]
    fn sanitize_turns_path_into_safe_filename() {
        assert_eq!(
            sanitize("/lol-gameflow/v1/gameflow-phase"),
            "lol-gameflow_v1_gameflow-phase"
        );
        assert_eq!(sanitize("/"), "root");
    }
}
