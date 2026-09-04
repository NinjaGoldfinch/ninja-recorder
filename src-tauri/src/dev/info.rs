//! Health and environment readouts for the portal's Overview panel.

use crate::{db, retention, state_machine, AppState};
use serde::Serialize;
use tauri::Manager;

#[derive(Serialize)]
pub struct DevEnvInfo {
    pub app_version: &'static str,
    pub identifier: String,
    pub os: &'static str,
    pub arch: &'static str,
    pub build_profile: &'static str,
    pub tauri_version: &'static str,
    pub recorder_backend: String,
    pub app_data_dir: String,
    pub recordings_dir: String,
    pub db_path: String,
    pub fixtures_dir: Option<String>,
    /// The repo's checked-in fixtures, only resolvable when running from
    /// source — an installed build's `CARGO_MANIFEST_DIR` points at a
    /// directory that exists only on the machine that compiled it.
    pub repo_fixtures_dir: Option<String>,
    pub sample_mp4_present: bool,
    pub lockfile_override: Option<String>,
    pub fixture_recording: bool,
}

#[tauri::command]
pub fn dev_env_info(state: tauri::State<AppState>, app: tauri::AppHandle) -> Result<DevEnvInfo, String> {
    let app_data_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let repo_fixtures = repo_fixtures_dir();

    Ok(DevEnvInfo {
        app_version: env!("CARGO_PKG_VERSION"),
        identifier: app.config().identifier.clone(),
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        build_profile: if cfg!(debug_assertions) { "debug" } else { "release" },
        tauri_version: tauri::VERSION,
        recorder_backend: state
            .recorder
            .lock()
            .map_err(|e| e.to_string())?
            .backend_name(),
        recordings_dir: state.recordings_dir.display().to_string(),
        db_path: app_data_dir.join("library.sqlite3").display().to_string(),
        fixtures_dir: crate::fixtures::base_dir().map(|d| d.display().to_string()),
        sample_mp4_present: repo_fixtures
            .as_ref()
            .is_some_and(|d| d.join("sample.mp4").exists()),
        repo_fixtures_dir: repo_fixtures.map(|d| d.display().to_string()),
        app_data_dir: app_data_dir.display().to_string(),
        lockfile_override: std::env::var("NINJA_RECORDER_LOCKFILE_PATH").ok(),
        fixture_recording: crate::fixtures::enabled(),
    })
}

/// The repo's own `fixtures/` directory, or `None` if the compiled-in path
/// no longer exists (an installed build, or a moved source tree).
pub(crate) fn repo_fixtures_dir() -> Option<std::path::PathBuf> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("fixtures");
    dir.exists().then(|| dir.canonicalize().unwrap_or(dir))
}

#[derive(Serialize)]
pub struct RowCounts {
    pub recordings: i64,
    pub markers: i64,
    pub samples: i64,
}

#[derive(Serialize)]
pub struct DevHealth {
    pub supervisor: state_machine::SupervisorStatus,
    pub session: Option<state_machine::DevSessionView>,
    pub is_recording: bool,
    pub total_bytes: i64,
    pub free_bytes: i64,
    pub counts: RowCounts,
    pub policy: db::RetentionPolicy,
    pub replay_running: bool,
    pub fixture_recording: bool,
}

/// Everything the Overview panel polls, in one round trip. Six separate
/// `invoke`s per tick at 1 Hz would be six IPC hops and six DB locks for a
/// display that is only ever read as a whole.
#[tauri::command]
pub fn dev_health(
    state: tauri::State<AppState>,
    dev: tauri::State<super::DevState>,
) -> Result<DevHealth, String> {
    let counts = {
        let conn = state.db.conn();
        let count = |table: &str| -> Result<i64, String> {
            conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                .map_err(|e| e.to_string())
        };
        RowCounts {
            recordings: count("recordings")?,
            markers: count("markers")?,
            samples: count("samples")?,
        }
    };

    Ok(DevHealth {
        supervisor: state.supervisor.status(),
        session: state.supervisor.dev_session_view(),
        is_recording: state
            .recorder
            .lock()
            .map_err(|e| e.to_string())?
            .is_recording(),
        total_bytes: state.db.total_size_bytes().map_err(|e| e.to_string())?,
        free_bytes: retention::free_space_bytes(&state.recordings_dir).unwrap_or(0) as i64,
        counts,
        policy: state.db.get_retention_policy().map_err(|e| e.to_string())?,
        replay_running: dev.replay.lock().map_err(|e| e.to_string())?.is_some(),
        fixture_recording: crate::fixtures::enabled(),
    })
}

/// Every command name registered in `lib.rs`'s production handler list.
/// The portal diffs this against its own TS registry and shows a banner on
/// mismatch — the closest thing to a drift check available while the TS
/// types are hand-mirrored rather than generated.
#[tauri::command]
pub fn dev_registered_commands() -> Vec<&'static str> {
    vec![
        "start_recording",
        "stop_recording",
        "is_recording",
        "list_recordings",
        "rescan_recordings",
        "get_recording_markers",
        "get_recording_samples",
        "get_disk_usage",
        "get_retention_policy",
        "set_retention_policy",
        "set_pinned",
        "lcu_status",
        "game_state_status",
    ]
}

/// Reveals one of the app's directories in the OS file manager.
#[tauri::command]
pub fn dev_open_data_dir(
    state: tauri::State<AppState>,
    app: tauri::AppHandle,
    which: String,
) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;

    let path = match which.as_str() {
        "recordings" => state.recordings_dir.clone(),
        "app_data" => app.path().app_data_dir().map_err(|e| e.to_string())?,
        "fixtures" => crate::fixtures::base_dir()
            .ok_or_else(|| "fixtures dir not initialized".to_string())?,
        "repo_fixtures" => {
            repo_fixtures_dir().ok_or_else(|| "no repo fixtures dir on this machine".to_string())?
        }
        other => return Err(format!("unknown directory: {other}")),
    };

    // Revealing a directory that doesn't exist yet silently does nothing
    // on some platforms, which reads as the button being broken.
    std::fs::create_dir_all(&path).map_err(|e| e.to_string())?;
    app.opener()
        .open_path(path.display().to_string(), None::<&str>)
        .map_err(|e| e.to_string())
}
