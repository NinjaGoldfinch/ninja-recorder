mod db;
#[cfg(feature = "devtools")]
mod dev;
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

/// Emitted whenever the VOD library changes behind the frontend's back —
/// a finalize, a retention deletion, or any dev-portal write. The library
/// view listens for it and re-fetches; without it a recording only
/// appeared after a manual Refresh.
pub(crate) const LIBRARY_CHANGED_EVENT: &str = "library-changed";

pub(crate) struct AppState {
    pub(crate) recorder: Arc<Mutex<Box<dyn Recorder>>>,
    pub(crate) supervisor: Arc<state_machine::Supervisor>,
    pub(crate) db: Arc<db::Db>,
    pub(crate) recordings_dir: std::path::PathBuf,
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

/// Advantage-curve samples for the review timeline's graph. Returns an
/// empty vec for any recording made before sampling existed — the frontend
/// treats that as "no metric data" rather than an error.
#[tauri::command]
fn get_recording_samples(
    state: tauri::State<AppState>,
    recording_id: i64,
) -> Result<Vec<db::SampleRow>, String> {
    state
        .db
        .get_samples(recording_id)
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
    app: tauri::AppHandle,
    policy: db::RetentionPolicy,
) -> Result<retention::EnforcementReport, String> {
    state
        .db
        .set_retention_policy(&policy)
        .map_err(|e| e.to_string())?;
    let report = retention::enforce_now(&state.db, &policy).map_err(|e| e.to_string())?;
    if !report.deleted.is_empty() {
        use tauri::Emitter;
        let _ = app.emit(LIBRARY_CHANGED_EVENT, ());
    }
    Ok(report)
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
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let backend: Box<dyn Recorder> = {
                #[cfg(target_os = "windows")]
                {
                    use tauri::path::BaseDirectory;
                    // Optional: only used to remux each recording to a
                    // seekable faststart MP4 on stop (see LibObsRecorder's
                    // `stop`) — `None` if unstaged rather than failing
                    // init, since recording itself doesn't depend on it.
                    let ffmpeg_path = app
                        .path()
                        .resolve("libobs/ffmpeg.exe", BaseDirectory::Resource)
                        .ok();
                    // Init failure here (missing/unstaged libobs files, no
                    // usable GPU, etc.) must not take the whole app down —
                    // only recording depends on this. Fall back to a
                    // recorder that surfaces the error on `start` instead
                    // of propagating it out of `setup`.
                    let init = app
                        .path()
                        .resolve("libobs/extprocess_recorder.exe", BaseDirectory::Resource)
                        .map_err(|e| e.to_string())
                        .and_then(|path| {
                            recorder::libobs::LibObsRecorder::new(path, ffmpeg_path)
                                .map_err(|e| e.to_string())
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
            // Which backend you get depends on target OS *and* on whether
            // libobs managed to initialize, and the difference decides
            // whether recording works at all — worth one line at startup
            // rather than only being discoverable by trying to record.
            println!("[recorder] backend: {}", backend.backend_name());

            let recorder: Arc<Mutex<Box<dyn Recorder>>> = Arc::new(Mutex::new(backend));
            let dir = recordings_dir(app.handle())?;

            // Must happen before the supervisor starts polling — see
            // fixtures::set_base_dir's doc comment.
            fixtures::set_base_dir(app.path().app_data_dir()?.join("fixtures"));
            fixtures::init_from_env();

            let db_path = app.path().app_data_dir()?.join("library.sqlite3");
            std::fs::create_dir_all(db_path.parent().expect("db path always has a parent"))?;
            let db = Arc::new(match db::Db::open(&db_path) {
                Ok(db) => db,
                // Returning `Err` here would hand this to Tauri's setup
                // hook, which `expect`s on it — and because that runs
                // inside a platform callback that can't unwind, the user
                // gets an abort and thirty frames of backtrace instead of
                // a reason. Every case is fatal (nothing in the app works
                // without the library), so print something actionable and
                // leave quietly.
                Err(e @ db::DbError::SchemaTooNew { .. }) => {
                    eprintln!(
                        "\n[db] cannot open the VOD library: {e}.\n\
                         \n  {}\n\
                         \nThis happens after switching to an older branch, or downgrading the\n\
                         app: migrations only run forward. The file is left untouched. Either go\n\
                         back to the newer build, or move that file aside to start a fresh\n\
                         library (its recordings stay on disk and are re-imported by the startup\n\
                         folder scan — only the metadata is lost).\n",
                        db_path.display()
                    );
                    std::process::exit(1);
                }
                Err(e) => {
                    eprintln!("\n[db] cannot open the VOD library at {}: {e}\n", db_path.display());
                    std::process::exit(1);
                }
            });

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
            supervisor.attach_app(app.handle().clone());
            supervisor.start();

            app.manage(AppState {
                recorder,
                supervisor,
                db,
                recordings_dir: dir,
            });
            #[cfg(feature = "devtools")]
            app.manage(dev::DevState::default());
            Ok(())
        });

    // `generate_handler!` takes a literal path list — it can't host a
    // `#[cfg]` attribute or a macro expansion inside the brackets — so the
    // two variants are spelled out. The production list must stay
    // identical between them; `dev_registered_commands` exists so the dev
    // portal can catch it if they ever drift.
    #[cfg(not(feature = "devtools"))]
    let builder = builder.invoke_handler(tauri::generate_handler![
        start_recording,
        stop_recording,
        is_recording,
        list_recordings,
        rescan_recordings,
        get_recording_markers,
        get_recording_samples,
        get_disk_usage,
        get_retention_policy,
        set_retention_policy,
        set_pinned,
        lcu_status,
        game_state_status
    ]);

    #[cfg(feature = "devtools")]
    let builder = builder.invoke_handler(tauri::generate_handler![
        start_recording,
        stop_recording,
        is_recording,
        list_recordings,
        rescan_recordings,
        get_recording_markers,
        get_recording_samples,
        get_disk_usage,
        get_retention_policy,
        set_retention_policy,
        set_pinned,
        lcu_status,
        game_state_status,
        dev::dev_open_portal,
        dev::dev_env_info,
        dev::dev_health,
        dev::dev_registered_commands,
        dev::dev_open_data_dir,
        dev::dev_schema,
        dev::dev_table_page,
        dev::dev_sql_query,
        dev::dev_insert_row,
        dev::dev_update_row,
        dev::dev_delete_row,
        dev::dev_reset_db,
        dev::dev_seed_library,
        dev::dev_clear_seeded,
        dev::dev_retention_preview,
        dev::dev_dispatch_state_event,
        dev::dev_inject_snapshot,
        dev::dev_session_snapshot,
        dev::dev_replay_start,
        dev::dev_replay_stop,
        dev::dev_replay_status,
        dev::dev_lcu_get,
        dev::dev_fetch_match_summary,
        dev::dev_live_client_probe,
        dev::dev_fixtures_state,
        dev::dev_fixture_read,
        dev::dev_fixture_write,
        dev::dev_set_fixture_recording
    ]);

    builder
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
