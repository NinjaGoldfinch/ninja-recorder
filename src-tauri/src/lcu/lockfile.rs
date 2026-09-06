//! LCU lockfile discovery + parsing. DEVELOPMENT.md §3.1.
//!
//! The League Client writes a `lockfile` next to its executable while
//! running, in the format `name:pid:port:password:protocol`. Its presence
//! is the source of truth for "is the client running"; its contents are
//! everything needed to talk to the LCU API.
//!
//! `watch` is driven continuously by the state machine's supervisor
//! (`state_machine::supervisor::Supervisor::start`); `discover` is also
//! used directly by the one-shot `lcu_status` dev command.

#[cfg(target_os = "windows")]
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockfileInfo {
    pub name: String,
    pub pid: u32,
    pub port: u16,
    pub password: String,
    pub protocol: String,
}

#[derive(Debug, thiserror::Error)]
pub enum LockfileError {
    #[error("malformed lockfile contents: {0:?}")]
    Malformed(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

impl LockfileInfo {
    /// Parses the `name:pid:port:password:protocol` format the League
    /// Client writes to its lockfile.
    pub fn parse(contents: &str) -> Result<Self, LockfileError> {
        let parts: Vec<&str> = contents.trim().split(':').collect();
        let [name, pid, port, password, protocol]: [&str; 5] = parts
            .try_into()
            .map_err(|_| LockfileError::Malformed(contents.to_string()))?;

        Ok(LockfileInfo {
            name: name.to_string(),
            pid: pid
                .parse()
                .map_err(|_| LockfileError::Malformed(contents.to_string()))?,
            port: port
                .parse()
                .map_err(|_| LockfileError::Malformed(contents.to_string()))?,
            password: password.to_string(),
            protocol: protocol.to_string(),
        })
    }

    pub fn base_url(&self) -> String {
        format!("{}://127.0.0.1:{}", self.protocol, self.port)
    }

    pub fn ws_url(&self) -> String {
        format!("wss://127.0.0.1:{}", self.port)
    }
}

/// Reads and parses the lockfile at `path`. Returns `Ok(None)` if the file
/// doesn't exist (client not running) — that's the expected steady state
/// most of the time, not an error condition.
pub fn read_lockfile(path: &Path) -> Result<Option<LockfileInfo>, LockfileError> {
    match std::fs::read_to_string(path) {
        Ok(contents) => LockfileInfo::parse(&contents).map(Some),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Candidate lockfile paths in priority order. `env_override` (wired to
/// `NINJA_RECORDER_LOCKFILE_PATH` by callers) always wins when set — useful
/// for tests and non-standard installs. After that: macOS checks the app
/// bundle's default location; Windows consults `RiotClientInstalls.json`
/// (installs can live on any drive) before falling back to the
/// conventional default install dir.
pub fn candidate_paths(env_override: Option<&str>) -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Some(path) = env_override {
        paths.push(PathBuf::from(path));
    }

    #[cfg(target_os = "macos")]
    {
        paths.push(PathBuf::from(
            "/Applications/League of Legends.app/Contents/LoL/lockfile",
        ));
    }

    #[cfg(target_os = "windows")]
    {
        if let Some(install_dir) = windows_install_dir_from_installs_json() {
            paths.push(install_dir.join("lockfile"));
        }
        paths.push(PathBuf::from(r"C:\Riot Games\League of Legends\lockfile"));
    }

    paths
}

#[cfg(target_os = "windows")]
#[derive(Deserialize)]
struct RiotClientInstalls {
    associated_client: std::collections::HashMap<String, String>,
}

/// Cached wrapper around `read_windows_install_dir`. `candidate_paths` is
/// called once per `watch` tick — every 2 s for the app's lifetime — and the
/// uncached lookup reads *and JSON-parses* `RiotClientInstalls.json` every
/// time, whether or not League is even installed. That is pure waste against
/// DEVELOPMENT.md §1.2's idle-CPU target.
///
/// A resolved install directory is cached for the process's lifetime: League
/// cannot move itself while we are running, and if it somehow did, the
/// lockfile under the old path simply stops appearing. A *failed* lookup is
/// only trusted for `MISS_TTL`, so installing League while the app is already
/// running is still picked up.
#[cfg(target_os = "windows")]
fn windows_install_dir_from_installs_json() -> Option<PathBuf> {
    use std::sync::Mutex;
    use std::time::Instant;

    const MISS_TTL: Duration = Duration::from_secs(60);
    static CACHE: Mutex<Option<(Instant, Option<PathBuf>)>> = Mutex::new(None);

    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((resolved_at, cached)) = cache.as_ref() {
        if cached.is_some() || resolved_at.elapsed() < MISS_TTL {
            return cached.clone();
        }
    }

    let fresh = read_windows_install_dir();
    *cache = Some((Instant::now(), fresh.clone()));
    fresh
}

#[cfg(target_os = "windows")]
fn read_windows_install_dir() -> Option<PathBuf> {
    let program_data = std::env::var("PROGRAMDATA").ok()?;
    let installs_path = PathBuf::from(program_data)
        .join("Riot Games")
        .join("RiotClientInstalls.json");
    let contents = std::fs::read_to_string(installs_path).ok()?;
    let installs: RiotClientInstalls = serde_json::from_str(&contents).ok()?;
    installs
        .associated_client
        .keys()
        .find(|path| path.to_lowercase().contains("league of legends"))
        .map(PathBuf::from)
}

/// Finds the first candidate path with a parseable lockfile.
pub fn discover() -> Result<Option<LockfileInfo>, LockfileError> {
    let env_override = std::env::var("NINJA_RECORDER_LOCKFILE_PATH").ok();
    for path in candidate_paths(env_override.as_deref()) {
        if let Some(info) = read_lockfile(&path)? {
            return Ok(Some(info));
        }
    }
    Ok(None)
}

/// A point-in-time snapshot used by the watch loop to detect transitions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockfileState {
    Absent,
    Present(LockfileInfo),
}

/// How long to wait before the next `discover()`, given how many polls in a
/// row have found no lockfile.
///
/// This watch loop is the app's only unconditional background work — it runs
/// from launch to exit whether or not League is installed, so at a flat 2 s it
/// is the floor under idle CPU (DEVELOPMENT.md §1.2). But it is also how the
/// state machine learns the client exists, so it cannot simply be slow: a
/// grace window keeps the base cadence across a client *restart* (the lockfile
/// vanishing and reappearing within a few seconds), and only a sustained
/// absence — nobody is playing — ramps down.
///
/// With the caller's 2 s base that is: 2 s for the first 5 misses, then
/// 4 s, 8 s, 16 s, and 30 s thereafter, reaching the cap ~38 s after the
/// client disappears. Pure so it can be tested without a clock.
fn poll_delay(base: Duration, consecutive_misses: u32) -> Duration {
    /// Misses tolerated at the base cadence before ramping down.
    const GRACE: u32 = 5;
    const CAP: Duration = Duration::from_secs(30);

    if consecutive_misses <= GRACE {
        return base;
    }
    // Shift is clamped well below u32's width; `base * factor` cannot
    // overflow for any sane base because of the `.min(CAP)`.
    let factor = 1u32 << (consecutive_misses - GRACE).min(6);
    base.saturating_mul(factor).min(CAP)
}

/// Polls for lockfile appear/disappear/change, invoking `on_change` whenever
/// the state actually transitions (not on every poll). `interval` is the base
/// cadence used while the client is running; see `poll_delay` for how it backs
/// off while the client is absent. Runs until the calling task is aborted —
/// callers spawn this via `tauri::async_runtime::spawn` and let it live for
/// the app's lifetime.
pub async fn watch<F>(interval: Duration, mut on_change: F)
where
    F: FnMut(LockfileState) + Send,
{
    let mut last: Option<LockfileState> = None;
    let mut misses: u32 = 0;
    loop {
        // Discover first, then sleep: `tokio::time::interval`'s first tick
        // completes immediately, and the state machine depends on learning
        // about an already-running client at startup rather than one
        // interval later.
        let current = match discover() {
            Ok(Some(info)) => LockfileState::Present(info),
            Ok(None) => LockfileState::Absent,
            // Transient read error (e.g. file mid-write) — treat as absent
            // for this tick, retry next tick rather than erroring out.
            Err(_) => LockfileState::Absent,
        };
        if matches!(current, LockfileState::Present(_)) {
            misses = 0;
        } else {
            misses = misses.saturating_add(1);
        }
        if last.as_ref() != Some(&current) {
            on_change(current.clone());
            last = Some(current);
        }
        tokio::time::sleep(poll_delay(interval, misses)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_well_formed_lockfile() {
        let info = LockfileInfo::parse("LeagueClient:12345:54321:abcdEFGH12345:https").unwrap();
        assert_eq!(info.name, "LeagueClient");
        assert_eq!(info.pid, 12345);
        assert_eq!(info.port, 54321);
        assert_eq!(info.password, "abcdEFGH12345");
        assert_eq!(info.protocol, "https");
    }

    #[test]
    fn rejects_malformed_lockfile() {
        assert!(LockfileInfo::parse("not:enough:fields").is_err());
        assert!(LockfileInfo::parse("").is_err());
        assert!(LockfileInfo::parse("a:b:c:d:e:f").is_err());
    }

    #[test]
    fn rejects_non_numeric_pid_or_port() {
        assert!(LockfileInfo::parse("LeagueClient:notanum:54321:pw:https").is_err());
        assert!(LockfileInfo::parse("LeagueClient:1:notanum:pw:https").is_err());
    }

    #[test]
    fn base_url_and_ws_url_use_loopback() {
        let info = LockfileInfo::parse("LeagueClient:1:2999:pw:https").unwrap();
        assert_eq!(info.base_url(), "https://127.0.0.1:2999");
        assert_eq!(info.ws_url(), "wss://127.0.0.1:2999");
    }

    #[test]
    fn read_lockfile_missing_file_returns_none() {
        let path = std::env::temp_dir().join("ninja-recorder-nonexistent-lockfile-test");
        assert_eq!(read_lockfile(&path).unwrap(), None);
    }

    #[test]
    fn read_lockfile_reads_real_file() {
        let path = std::env::temp_dir().join(format!(
            "ninja-recorder-lockfile-test-{}-{}",
            std::process::id(),
            line!()
        ));
        std::fs::write(&path, "LeagueClient:1:2999:pw:https").unwrap();
        let info = read_lockfile(&path).unwrap().unwrap();
        assert_eq!(info.port, 2999);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn candidate_paths_env_override_takes_priority() {
        let paths = candidate_paths(Some("/tmp/whatever-lockfile"));
        assert_eq!(paths[0], PathBuf::from("/tmp/whatever-lockfile"));
    }

    #[test]
    fn candidate_paths_without_override_has_platform_defaults() {
        let paths = candidate_paths(None);
        assert!(!paths.is_empty(), "expected at least one platform default path");
    }

    #[test]
    fn poll_delay_holds_base_cadence_while_the_client_is_present() {
        let base = Duration::from_secs(2);
        assert_eq!(poll_delay(base, 0), base);
    }

    #[test]
    fn poll_delay_holds_base_cadence_through_a_client_restart() {
        // A client restart makes the lockfile vanish for a few polls. Backing
        // off during that window would delay noticing it came back.
        let base = Duration::from_secs(2);
        for misses in 1..=5 {
            assert_eq!(poll_delay(base, misses), base, "miss {misses}");
        }
    }

    #[test]
    fn poll_delay_ramps_down_once_the_client_is_sustainedly_absent() {
        let base = Duration::from_secs(2);
        assert_eq!(poll_delay(base, 6), Duration::from_secs(4));
        assert_eq!(poll_delay(base, 7), Duration::from_secs(8));
        assert_eq!(poll_delay(base, 8), Duration::from_secs(16));
    }

    #[test]
    fn poll_delay_is_capped_and_never_overflows() {
        let base = Duration::from_secs(2);
        let cap = Duration::from_secs(30);
        assert_eq!(poll_delay(base, 9), cap);
        assert_eq!(poll_delay(base, 100), cap);
        assert_eq!(poll_delay(base, u32::MAX), cap);
        // A pathological base must still clamp rather than panic.
        assert_eq!(poll_delay(Duration::MAX, u32::MAX), cap);
    }
}
