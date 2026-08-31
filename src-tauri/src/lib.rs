mod lcu;
mod recorder;

use recorder::{stub::StubRecorder, RecordConfig, Recorder};
use std::sync::Mutex;
use tauri::Manager;

struct AppState {
    recorder: Mutex<Box<dyn Recorder>>,
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

/// Placeholder for Phase 4 (SQLite VOD library). Returns what's actually on
/// disk in the recordings dir so the Phase 1 shell has something real to
/// list, without a database yet.
#[tauri::command]
fn list_recordings(app: tauri::AppHandle) -> Result<Vec<String>, String> {
    let dir = recordings_dir(&app)?;
    if !dir.exists() {
        return Ok(vec![]);
    }
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .map_err(|e| e.to_string())?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    names.sort();
    Ok(names)
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

/// Timestamp for default filenames. Not a general-purpose clock — swapped
/// for real match metadata (game id, champion) once the state machine
/// (Phase 3) drives recording from gameflow events instead of a manual
/// button.
fn chrono_stamp() -> u128 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(AppState {
            recorder: Mutex::new(Box::new(StubRecorder::new())),
        })
        .invoke_handler(tauri::generate_handler![
            start_recording,
            stop_recording,
            is_recording,
            list_recordings,
            lcu_status
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
