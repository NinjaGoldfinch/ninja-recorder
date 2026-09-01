mod db;
mod fixtures;
mod lcu;
mod live_client;
mod recorder;
mod retention;
mod state_machine;

#[cfg(not(target_os = "windows"))]
use recorder::stub::StubRecorder;
use recorder::{RecordConfig, Recorder};
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
    let dir = recordings_dir(&app)?;
    if !retention::has_room_to_record(&dir) {
        return Err("Not enough free disk space to start recording".to_string());
    }
    let config = RecordConfig {
        output_dir: dir,
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
struct DiskUsage {
    total_bytes: i64,
    recording_count: i64,
    free_bytes: i64,
}

/// Usage summary for the library UI (Phase 9 / DEVELOPMENT.md §6) — shown
/// alongside the retention policy so nothing gets deleted as a surprise.
#[tauri::command]
fn get_disk_usage(state: tauri::State<AppState>) -> Result<DiskUsage, String> {
    let total_bytes = state.db.total_size_bytes().map_err(|e| e.to_string())?;
    let recording_count = state.db.list_recordings().map_err(|e| e.to_string())?.len() as i64;
    let free_bytes = retention::free_space_bytes(&state.recordings_dir).unwrap_or(0) as i64;
    Ok(DiskUsage {
        total_bytes,
        recording_count,
        free_bytes,
    })
}

#[tauri::command]
fn get_retention_policy(state: tauri::State<AppState>) -> Result<db::RetentionPolicy, String> {
    state.db.get_retention_policy().map_err(|e| e.to_string())
}

/// Saves the policy and immediately re-enforces it — otherwise a
/// newly-tightened limit wouldn't take effect until the next finalize or
/// app restart, which would leave the UI's own usage number stale.
#[tauri::command]
fn set_retention_policy(
    state: tauri::State<AppState>,
    policy: db::RetentionPolicy,
) -> Result<retention::EnforcementReport, String> {
    state
        .db
        .set_retention_policy(&policy)
        .map_err(|e| e.to_string())?;
    retention::enforce_now(&state.db, &policy).map_err(|e| e.to_string())
}

#[tauri::command]
fn set_pinned(state: tauri::State<AppState>, recording_id: i64, pinned: bool) -> Result<(), String> {
    state
        .db
        .set_pinned(recording_id, pinned)
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
            let backend: Box<dyn Recorder> = {
                #[cfg(target_os = "windows")]
                {
                    use tauri::path::BaseDirectory;
                    // Init failure here (missing/unstaged libobs files, no
                    // usable GPU, etc.) must not take the whole app down —
                    // only recording depends on this. Fall back to a
                    // recorder that surfaces the error on `start` instead
                    // of propagating it out of `setup`.
                    let init = app
                        .path()
                        .resolve("libobs/extprocess_recorder.exe", BaseDirectory::Executable)
                        .map_err(|e| e.to_string())
                        .and_then(|path| {
                            recorder::libobs::LibObsRecorder::new(path).map_err(|e| e.to_string())
                        });
                    match init {
                        Ok(recorder) => Box::new(recorder),
                        Err(e) => {
                            eprintln!(
                                "[recorder] libobs backend failed to initialize, recording disabled: {e}"
                            );
                            Box::new(recorder::FailedRecorder(e))
                        }
                    }
                }
                #[cfg(not(target_os = "windows"))]
                {
                    Box::new(StubRecorder::new())
                }
            };
            let recorder: Arc<Mutex<Box<dyn Recorder>>> = Arc::new(Mutex::new(backend));
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

            // Retention (Phase 9 / DEVELOPMENT.md §6): enforced here and
            // again after every finalize (state_machine::supervisor), so a
            // policy set while the app was closed — or last session's
            // finalize enforcement never running because the app crashed
            // — still gets applied on the next launch.
            match db.get_retention_policy() {
                Ok(policy) => match retention::enforce_now(&db, &policy) {
                    Ok(report) if !report.deleted.is_empty() => println!(
                        "[retention] startup enforcement: removed {} recording(s), freed {} bytes",
                        report.deleted.len(),
                        report.freed_bytes
                    ),
                    Ok(_) => {}
                    Err(e) => eprintln!("[retention] startup enforcement failed: {e}"),
                },
                Err(e) => eprintln!("[retention] failed to load policy: {e}"),
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
            get_disk_usage,
            get_retention_policy,
            set_retention_policy,
            set_pinned,
            lcu_status,
            game_state_status
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
