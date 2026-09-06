mod audio_tracks;
mod core;
mod db;
#[cfg(feature = "devtools")]
mod dev;
mod fixtures;
mod lcu;
mod live_client;
mod recorder;
mod retention;
mod state_machine;

use recorder::audio::{AudioInputDevice, AudioPreset};
#[cfg(not(target_os = "windows"))]
use recorder::stub::StubRecorder;
use recorder::Recorder;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tauri::Manager;

/// Emitted whenever the VOD library changes behind the frontend's back —
/// a finalize, a retention deletion, or any dev-portal write. The library
/// view listens for it and re-fetches; without it a recording only
/// appeared after a manual Refresh.
pub(crate) const LIBRARY_CHANGED_EVENT: &str = "library-changed";

/// Tauri's managed state: a handle on the `core::Ctx` that actually holds
/// everything.
///
/// A newtype rather than `Ctx` directly because `Ctx` must stay free of
/// `tauri` types (see `core`'s header), and this is the boundary where the
/// two meet. It `Deref`s to `Ctx`, so the dev portal's many `state.db` /
/// `state.recorder` / `state.supervisor` field reads keep working unchanged.
pub(crate) struct AppState(pub(crate) Arc<core::Ctx>);

impl std::ops::Deref for AppState {
    type Target = core::Ctx;

    fn deref(&self) -> &core::Ctx {
        &self.0
    }
}

impl AppState {
    /// The explicit spelling, for the command wrappers — clearer at the call
    /// site than relying on deref coercion through `tauri::State`.
    pub(crate) fn ctx(&self) -> &core::Ctx {
        &self.0
    }

    /// A cheap owned handle, for the commands that hand work to a blocking
    /// thread and so can't hold a borrow of managed state across an await.
    pub(crate) fn clone_ctx(&self) -> Arc<core::Ctx> {
        Arc::clone(&self.0)
    }
}

fn recordings_dir(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|dir| dir.join("recordings"))
        .map_err(|e| e.to_string())
}

/// The bundled ffmpeg, if it was staged into this build.
///
/// Optional by design and in two places at once: `LibObsRecorder::stop` uses
/// it to remux each recording to a seekable file, and `extract_audio_track`
/// uses it to pull out an audio stem. Neither is allowed to be a hard
/// dependency — a failed download in CI degrades those features rather than
/// breaking recording — so both resolve it the same way and handle `None`.
fn ffmpeg_path(app: &tauri::AppHandle) -> Option<std::path::PathBuf> {
    #[cfg(target_os = "windows")]
    {
        app.path()
            .resolve("libobs/ffmpeg.exe", tauri::path::BaseDirectory::Resource)
            .ok()
    }
    #[cfg(not(target_os = "windows"))]
    {
        // Nothing is bundled off Windows, but a locally installed ffmpeg
        // makes stem extraction testable in the macOS dev loop.
        let _ = app;
        which_ffmpeg()
    }
}

#[cfg(not(target_os = "windows"))]
fn which_ffmpeg() -> Option<std::path::PathBuf> {
    ["/opt/homebrew/bin/ffmpeg", "/usr/local/bin/ffmpeg", "/usr/bin/ffmpeg"]
        .into_iter()
        .map(std::path::PathBuf::from)
        .find(|p| p.exists())
}

// Every command below is a thin wrapper over `core`, which holds the actual
// logic and names no `tauri` type. See that module's header for why the
// split exists. Two commands are *not* wrappers — `open_recordings_folder`
// here and `dev_open_portal` in `dev/` — because they drive the desktop
// shell, which only a UI process can do.

#[tauri::command]
fn start_recording(state: tauri::State<AppState>) -> Result<(), String> {
    core::start_recording(state.ctx())
}

#[tauri::command]
fn stop_recording(state: tauri::State<AppState>) -> Result<String, String> {
    core::stop_recording(state.ctx())
}

#[tauri::command]
fn is_recording(state: tauri::State<AppState>) -> Result<bool, String> {
    core::is_recording(state.ctx())
}

#[tauri::command]
fn list_recordings(state: tauri::State<AppState>) -> Result<Vec<db::RecordingRow>, String> {
    core::list_recordings(state.ctx())
}

#[tauri::command]
fn rescan_recordings(
    state: tauri::State<AppState>,
) -> Result<db::reconcile::ReconcileReport, String> {
    core::rescan_recordings(state.ctx())
}

#[tauri::command]
fn get_recording_markers(
    state: tauri::State<AppState>,
    recording_id: i64,
) -> Result<Vec<db::MarkerRow>, String> {
    core::get_recording_markers(state.ctx(), recording_id)
}

#[tauri::command]
fn get_recording_samples(
    state: tauri::State<AppState>,
    recording_id: i64,
) -> Result<Vec<db::SampleRow>, String> {
    core::get_recording_samples(state.ctx(), recording_id)
}

#[tauri::command]
fn get_disk_usage(state: tauri::State<AppState>) -> Result<core::DiskUsage, String> {
    core::get_disk_usage(state.ctx())
}

#[tauri::command]
fn get_retention_policy(state: tauri::State<AppState>) -> Result<db::RetentionPolicy, String> {
    core::get_retention_policy(state.ctx())
}

#[tauri::command]
fn set_retention_policy(
    state: tauri::State<AppState>,
    policy: db::RetentionPolicy,
) -> Result<retention::EnforcementReport, String> {
    core::set_retention_policy(state.ctx(), policy)
}

#[tauri::command]
fn set_pinned(
    state: tauri::State<AppState>,
    recording_id: i64,
    pinned: bool,
) -> Result<(), String> {
    core::set_pinned(state.ctx(), recording_id, pinned)
}

#[tauri::command]
fn preview_retention_policy(
    state: tauri::State<AppState>,
    policy: db::RetentionPolicy,
) -> Result<retention::EnforcementReport, String> {
    core::preview_retention_policy(state.ctx(), policy)
}

#[tauri::command]
fn delete_recording(state: tauri::State<AppState>, recording_id: i64) -> Result<(), String> {
    core::delete_recording(state.ctx(), recording_id)
}

#[tauri::command]
fn get_recordings_dir(state: tauri::State<AppState>) -> String {
    core::get_recordings_dir(state.ctx())
}

/// Reveals the recordings folder in Finder/Explorer. **Not** a `core`
/// wrapper: this drives the desktop shell, so it stays in the UI process
/// when the recorder moves into its own.
///
/// Deliberately done here rather than from the frontend with
/// `@tauri-apps/plugin-opener`: the JS `openPath` command is gated on the
/// opener scope, which is empty, so it always denies. The Rust function is
/// a plain call with no ACL involved. The folder is only created on the
/// first recording, so create it first — `open_path` stats the path and
/// fails on a fresh install otherwise.
#[tauri::command]
fn open_recordings_folder(state: tauri::State<AppState>) -> Result<(), String> {
    std::fs::create_dir_all(&state.recordings_dir).map_err(|e| e.to_string())?;
    tauri_plugin_opener::open_path(&state.recordings_dir, None::<&str>).map_err(|e| e.to_string())
}

#[tauri::command]
fn get_ui_prefs(state: tauri::State<AppState>) -> Result<HashMap<String, String>, String> {
    core::get_ui_prefs(state.ctx())
}

#[tauri::command]
fn set_ui_pref(state: tauri::State<AppState>, key: String, value: String) -> Result<(), String> {
    core::set_ui_pref(state.ctx(), &key, &value)
}

#[tauri::command]
fn get_audio_preset(state: tauri::State<AppState>) -> Result<AudioPreset, String> {
    core::get_audio_preset(state.ctx())
}

#[tauri::command]
fn set_audio_preset(state: tauri::State<AppState>, preset: AudioPreset) -> Result<(), String> {
    core::set_audio_preset(state.ctx(), preset)
}

/// `core::list_audio_inputs` is blocking, so it gets a thread of its own
/// here rather than stalling the command runtime — the daemon will make the
/// same call from its own per-request task.
#[tauri::command]
async fn list_audio_inputs() -> Result<Vec<AudioInputDevice>, String> {
    tauri::async_runtime::spawn_blocking(core::list_audio_inputs)
        .await
        .map_err(|e| e.to_string())?
}

/// Blocking for the same reason as `list_audio_inputs` — and more so, since
/// this one shells out to ffmpeg.
#[tauri::command]
async fn extract_audio_track(
    state: tauri::State<'_, AppState>,
    recording_path: String,
    track_index: usize,
) -> Result<String, String> {
    let ctx = state.clone_ctx();
    tauri::async_runtime::spawn_blocking(move || {
        core::extract_audio_track(&ctx, &recording_path, track_index)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn lcu_status() -> core::LcuStatus {
    core::lcu_status().await
}

#[tauri::command]
fn game_state_status(state: tauri::State<AppState>) -> state_machine::SupervisorStatus {
    core::game_state_status(state.ctx())
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
                    let ffmpeg = ffmpeg_path(app.handle());
                    // Only the *path* can fail here now. libobs itself no
                    // longer starts during setup — it comes up when the
                    // state machine sees the League client, and reports its
                    // own failure through `backend_name`/`start`. Either
                    // way this must not take the whole app down: the
                    // library, review UI and LCU polling don't need capture.
                    match app
                        .path()
                        .resolve("libobs/extprocess_recorder.exe", BaseDirectory::Resource)
                    {
                        Ok(path) => Box::new(recorder::libobs::LibObsRecorder::new(path, ffmpeg)),
                        Err(e) => {
                            eprintln!(
                                "[recorder] could not locate the libobs worker, recording disabled: {e}"
                            );
                            Box::new(recorder::FailedRecorder(e.to_string()))
                        }
                    }
                }
                #[cfg(not(target_os = "windows"))]
                {
                    Box::new(StubRecorder::new())
                }
            };
            // Which backend you get depends on the target OS and on
            // whether the worker binary was found, and the difference
            // decides whether recording works at all — worth one line at
            // startup rather than only being discoverable by trying to
            // record. On Windows this now says "idle" rather than "ready":
            // libobs comes up with the League client, not with the app.
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

            // Retention (DEVELOPMENT.md §6): enforced here and
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
            // The emit lives here, not in the supervisor: `run()` is dead
            // code in a `cargo test` build and gets stripped, which keeps
            // Tauri's Wry window machinery — and with it the Win32 GUI
            // import stack — out of the test binary. See
            // `Supervisor::on_library_changed` for what happens when it
            // isn't kept out.
            let notify_handle = app.handle().clone();
            supervisor.set_library_changed_notifier(Box::new(move || {
                use tauri::Emitter;
                if let Err(e) = notify_handle.emit(LIBRARY_CHANGED_EVENT, ()) {
                    eprintln!("[state_machine] failed to emit library-changed: {e}");
                }
            }));
            supervisor.start();

            // `ffmpeg` and `recordings_dir` are resolved once here rather
            // than per call from an `AppHandle`, which is what the three
            // commands that used to take one were doing — and is what lets
            // `core` stay free of `tauri` types.
            let mut ctx = core::Ctx::new(
                recorder,
                supervisor,
                db,
                dir,
                ffmpeg_path(app.handle()),
            );
            let notify_handle = app.handle().clone();
            ctx.set_library_changed_notifier(Box::new(move || {
                use tauri::Emitter;
                if let Err(e) = notify_handle.emit(LIBRARY_CHANGED_EVENT, ()) {
                    eprintln!("[core] failed to emit library-changed: {e}");
                }
            }));

            app.manage(AppState(Arc::new(ctx)));
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
        preview_retention_policy,
        delete_recording,
        get_recordings_dir,
        open_recordings_folder,
        get_ui_prefs,
        set_ui_pref,
        get_audio_preset,
        set_audio_preset,
        list_audio_inputs,
        extract_audio_track,
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
        preview_retention_policy,
        delete_recording,
        get_recordings_dir,
        open_recordings_folder,
        get_ui_prefs,
        set_ui_pref,
        get_audio_preset,
        set_audio_preset,
        list_audio_inputs,
        extract_audio_track,
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
