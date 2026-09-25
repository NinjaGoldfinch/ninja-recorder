//! Health and environment readouts for the portal's Overview panel.

use crate::{db, retention, state_machine, AppState};
use serde::Serialize;

/// Build and paths, as **this** process resolved them. It runs in the UI, so
/// it carries nothing about the recorder: the UI's is a `FailedRecorder`, and
/// reporting its name here is how the portal once described a process that
/// does not record (#282). The daemon's is in `DevHealth::recorder`.
#[derive(Serialize)]
pub struct DevEnvInfo {
    pub app_version: &'static str,
    pub identifier: String,
    pub os: &'static str,
    pub arch: &'static str,
    pub build_profile: &'static str,
    pub tauri_version: &'static str,
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
    // `Paths::resolve`, the function both processes use, so this shows the
    // folder the library is really in rather than what Tauri's config says.
    let paths = crate::daemon::Paths::resolve().map_err(|e| e.to_string())?;
    let repo_fixtures = repo_fixtures_dir();

    Ok(DevEnvInfo {
        app_version: env!("CARGO_PKG_VERSION"),
        identifier: app.config().identifier.clone(),
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        build_profile: if cfg!(debug_assertions) { "debug" } else { "release" },
        tauri_version: tauri::VERSION,
        recordings_dir: state.recordings_dir.display().to_string(),
        db_path: paths.db.display().to_string(),
        fixtures_dir: crate::fixtures::base_dir().map(|d| d.display().to_string()),
        sample_mp4_present: repo_fixtures
            .as_ref()
            .is_some_and(|d| d.join("sample.mp4").exists()),
        repo_fixtures_dir: repo_fixtures.map(|d| d.display().to_string()),
        app_data_dir: paths.data.display().to_string(),
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

/// The daemon's recorder, as the portal's Overview and Recorder panels show
/// it (#282). Read in the process that owns the `Recorder`, which is the
/// whole point: `dev_env_info` answers from the UI process, whose recorder is
/// a `FailedRecorder` that refuses everything.
#[derive(Serialize)]
pub struct DevRecorderView {
    /// `Recorder::backend_name` of the backend actually live, e.g.
    /// `own (ready: NVIDIA ...)` or `libobs (idle)`.
    pub backend: String,
    /// The `capture_backend` setting: `libobs`, `own`, or `unset` when
    /// nothing is saved (the daemon then picks own where it can be built,
    /// else libobs). A saved one can differ from the live backend only when
    /// the build cannot construct it, and then `backend` says why.
    pub configured: String,
    /// The file the recording in flight is written to.
    pub current_file: Option<String>,
    /// Whether the backend's capture worker process is up; `None` for a
    /// backend that has none.
    pub worker_running: Option<bool>,
}

#[derive(Serialize)]
pub struct DevHealth {
    pub supervisor: state_machine::SupervisorStatus,
    pub session: Option<state_machine::DevSessionView>,
    /// Whether the daemon's recorder is capturing. Kept at the top level
    /// rather than inside `recorder`, because the top bar's pill reads it.
    pub is_recording: bool,
    pub recorder: DevRecorderView,
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
pub fn dev_health(
    ctx: &crate::core::Ctx,
    
) -> Result<DevHealth, String> {
    let counts = {
        let conn = ctx.db.conn();
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

    let configured = ctx
        .db
        .get_capture_backend()
        .map_err(|e| e.to_string())?
        .map_or("unset", crate::recorder::backend::CaptureBackend::as_pref);
    // One lock for every recorder field, so they describe the same moment.
    // Taken after the supervisor's status is read, never around it: the
    // supervisor takes its own locks and then this one.
    let supervisor = ctx.supervisor.status();
    let session = ctx.supervisor.dev_session_view();
    let (is_recording, recorder) = {
        let recorder = ctx.recorder.lock().map_err(|e| e.to_string())?;
        (
            recorder.is_recording(),
            DevRecorderView {
                backend: recorder.backend_name(),
                configured: configured.to_string(),
                current_file: recorder.current_file().map(|p| p.display().to_string()),
                worker_running: recorder.worker_running(),
            },
        )
    };

    Ok(DevHealth {
        supervisor,
        session,
        is_recording,
        recorder,
        total_bytes: ctx.db.total_size_bytes().map_err(|e| e.to_string())?,
        free_bytes: retention::free_space_bytes(&ctx.recordings_dir).unwrap_or(0) as i64,
        counts,
        policy: ctx.db.get_retention_policy().map_err(|e| e.to_string())?,
        replay_running: super::dev_state().replay.lock().map_err(|e| e.to_string())?.is_some(),
        fixture_recording: crate::fixtures::enabled(),
    })
}

/// Every command name the production surface answers to. The portal diffs
/// this against its own TS registry and shows a banner on mismatch.
///
/// Derived from `core::command_names()` rather than hand-written, so the Rust
/// half can no longer drift: those names and the dispatch `match` arms come
/// out of the same macro invocation. `open_recordings_folder` is appended
/// because it is a real Tauri command rather than a dispatch-table entry — it
/// drives the desktop shell (DEVELOPMENT.md §12). `src/dev/registry.ts` stays
/// hand-maintained; it carries help text and argument specs no macro can
/// produce, and catching *its* drift is the point of the banner.
#[tauri::command]
pub fn dev_registered_commands() -> Vec<&'static str> {
    let mut names = crate::core::command_names().to_vec();
    names.push("open_recordings_folder");
    names.sort_unstable();
    names
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
        "app_data" => crate::daemon::Paths::resolve().map_err(|e| e.to_string())?.data,
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
