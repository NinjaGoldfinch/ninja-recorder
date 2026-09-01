mod db;
mod fixtures;
mod lcu;
mod live_client;
mod recorder;
mod state_machine;

use recorder::{stub::StubRecorder, RecordConfig, Recorder};
use std::sync::{Arc, Mutex};
use tauri::Manager;

struct AppState {
    recorder: Arc<Mutex<Box<dyn Recorder>>>,
    supervisor: Arc<state_machine::Supervisor>,
    db: Arc<db::Db>,
    recordings_dir: std::path::PathBuf,
}

fn recordings_dir(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|dir| dir.join("recordings"))
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn start_recording(state: tauri::State<AppState>, app: tauri::AppHandle) -> Result<(), String> {
    let config = RecordConfig {
        output_dir: recordings_dir(&app)?,
        file_stem: format!("recording-{}", chrono_stamp()),
    };
    state
        .recorder
        .lock()
        .map_err(|e| e.to_string())?
        .start(config)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn stop_recording(state: tauri::State<AppState>) -> Result<String, String> {
    let path = state
        .recorder
        .lock()
        .map_err(|e| e.to_string())?
        .stop()
        .map_err(|e| e.to_string())?;
    Ok(path.display().to_string())
}

#[tauri::command]
fn is_recording(state: tauri::State<AppState>) -> Result<bool, String> {
    Ok(state
        .recorder
        .lock()
        .map_err(|e| e.to_string())?
        .is_recording())
}

#[tauri::command]
fn list_recordings(state: tauri::State<AppState>) -> Result<Vec<db::RecordingRow>, String> {
    state.db.list_recordings().map_err(|e| e.to_string())
}

/// Re-runs folder-scan reconciliation on demand (also runs once at
/// startup). DEVELOPMENT.md §4 — "the library must survive users touching
/// the folder."
#[tauri::command]
fn rescan_recordings(state: tauri::State<AppState>) -> Result<db::reconcile::ReconcileReport, String> {
    db::reconcile::reconcile(&state.db, &state.recordings_dir).map_err(|e| e.to_string())
}

/// Markers for the review timeline (Phase 5).
#[tauri::command]
fn get_recording_markers(
    state: tauri::State<AppState>,
    recording_id: i64,
) -> Result<Vec<db::MarkerRow>, String> {
    state
        .db
        .get_markers(recording_id)
        .map_err(|e| e.to_string())
}

#[derive(serde::Serialize)]
struct LcuStatus {
    connected: bool,
    phase: Option<String>,
    summoner: Option<String>,
    error: Option<String>,
}

/// One-shot LCU status check for the dev panel: is the client running, and
/// if so, what's its current gameflow phase / summoner. Phase 3's state
/// machine is what keeps this live continuously via `lcu::gameflow::watch`;
/// this command is just a smoke test that the client + auth + parsing work.
#[tauri::command]
async fn lcu_status() -> LcuStatus {
    let lockfile = match lcu::lockfile::discover() {
        Ok(Some(lf)) => lf,
        Ok(None) => {
            return LcuStatus {
                connected: false,
                phase: None,
                summoner: None,
                error: None,
            }
        }
        Err(e) => {
            return LcuStatus {
                connected: false,
                phase: None,
                summoner: None,
                error: Some(e.to_string()),
            }
        }
    };

    let client = match lcu::LcuHttpClient::new(&lockfile) {
        Ok(c) => c,
        Err(e) => {
            return LcuStatus {
                connected: false,
                phase: None,
                summoner: None,
                error: Some(e.to_string()),
            }
        }
    };

    let phase = client
        .get_json::<lcu::GameflowPhase>("/lol-gameflow/v1/gameflow-phase")
        .await;
    let summoner = client
        .get_json::<lcu::match_data::CurrentSummoner>("/lol-summoner/v1/current-summoner")
        .await;

    LcuStatus {
        connected: true,
        phase: phase.ok().map(|p| format!("{:?}", p)),
        summoner: summoner.ok().map(|s| s.display_name),
        error: None,
    }
}

/// Timestamp for default filenames. Only used by the manual dev-panel
/// start button now — the state machine (Phase 3) has its own copy since
/// it drives recording independently, from gameflow events rather than a
/// button click.
fn chrono_stamp() -> u128 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// Current game state and the most recently finished recording (if any),
/// for the dev panel. The real, always-on driver is `Supervisor::start`,
/// spawned once at app startup below — this command just reads its status.
#[tauri::command]
fn game_state_status(state: tauri::State<AppState>) -> state_machine::SupervisorStatus {
    state.supervisor.status()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let recorder: Arc<Mutex<Box<dyn Recorder>>> =
                Arc::new(Mutex::new(Box::new(StubRecorder::new())));
            let dir = recordings_dir(app.handle())?;

            // Must happen before the supervisor starts polling — see
            // fixtures::set_base_dir's doc comment.
            fixtures::set_base_dir(app.path().app_data_dir()?.join("fixtures"));

            let db_path = app.path().app_data_dir()?.join("library.sqlite3");
            std::fs::create_dir_all(db_path.parent().expect("db path always has a parent"))?;
            let db = Arc::new(db::Db::open(&db_path)?);

            match db::reconcile::reconcile(&db, &dir) {
                Ok(report) if report.orphans_removed > 0 || report.imported > 0 => {
                    println!(
                        "[db] startup reconcile: removed {} orphan row(s), imported {} untracked file(s)",
                        report.orphans_removed, report.imported
                    );
                }
                Ok(_) => {}
                Err(e) => eprintln!("[db] startup reconcile failed: {e}"),
            }

            let supervisor =
                state_machine::Supervisor::new(Arc::clone(&recorder), dir.clone(), Arc::clone(&db));
            supervisor.start();

            app.manage(AppState {
                recorder,
                supervisor,
                db,
                recordings_dir: dir,
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            start_recording,
            stop_recording,
            is_recording,
            list_recordings,
            rescan_recordings,
            get_recording_markers,
            lcu_status,
            game_state_status
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
