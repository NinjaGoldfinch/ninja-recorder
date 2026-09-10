mod audio_tracks;
mod backfill;
mod core;
mod db;
mod ddragon;
#[cfg(feature = "devtools")]
mod dev;
mod fixtures;
mod launch;
mod notify;
mod lcu;
mod live_client;
mod log;
mod match_summary;
mod probe;
mod recorder;
mod retention;
mod state_machine;
mod tray;
mod trim;
mod update;

// No `use crate::{error, warn, info}` here, unlike every other module:
// this file *is* the crate root, and `#[macro_export]` already puts the
// macros in its macro namespace. Importing them would collide with the
// definitions themselves (E0255).
#[cfg(not(target_os = "windows"))]
use recorder::stub::StubRecorder;
use recorder::Recorder;
use std::sync::{Arc, Mutex};
use tauri::Manager;

/// Emitted whenever the VOD library changes behind the frontend's back —
/// a finalize, a retention deletion, or any dev-portal write. The library
/// view listens for it and re-fetches; without it a recording only
/// appeared after a manual Refresh.
pub(crate) const LIBRARY_CHANGED_EVENT: &str = "library-changed";

/// Emitted when the background update check has a new answer. The About
/// block and the settings badge listen for it, which is what keeps the
/// frontend from polling a question whose answer changes twice a day.
pub(crate) const UPDATE_STATUS_EVENT: &str = "update-status-changed";

/// How long after startup the first update check runs. Late enough that it
/// is never competing with the recorder backend coming up, the database
/// opening or the first paint — none of which should wait on a network
/// round-trip to GitHub.
const UPDATE_FIRST_CHECK_DELAY: std::time::Duration = std::time::Duration::from_secs(30);

/// And how often after that. Deliberately slack: CI publishes a release for
/// every commit that lands on `main`, so "something newer exists" is true
/// most days, and a tighter loop would only re-discover the same answer.
const UPDATE_CHECK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(6 * 60 * 60);

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
    /// A cheap owned handle. `rpc` needs this rather than a borrow: it hands
    /// the context to a blocking thread, and managed state can't be borrowed
    /// across an await.
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

/// An ffmpeg invocation that does not flash a console window.
///
/// ffmpeg is a console-subsystem binary, so Windows hands it a brand new
/// console whenever a GUI process spawns it — a black terminal window sitting
/// over the game for the length of every faststart remux, and again for every
/// stem extraction. Both callers pipe stdout and stderr, so that window never
/// had anything to show in the first place. Every spawn of the bundled ffmpeg
/// goes through here; there is no second way to launch it.
pub(crate) fn ffmpeg_command(path: &std::path::Path) -> std::process::Command {
    // The `mut` is only needed by the Windows branch below, and clippy runs
    // with `-D warnings` on a Linux runner too.
    #[cfg_attr(not(target_os = "windows"), allow(unused_mut))]
    let mut command = std::process::Command::new(path);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        // Spelled out rather than taken from the `windows` crate: it lives
        // behind `Win32_System_Threading`, a feature this build does not
        // otherwise need, and the value is fixed ABI.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// The whole production command surface, as one command.
///
/// `generate_handler!` takes a literal list and cannot host a `#[cfg]`, which
/// is why the production and devtools lists below used to spell out the same
/// 23 names twice — and `dev_registered_commands` a third time. Routing
/// everything through `core`'s dispatch table collapses all of that: the names
/// now come from `core::command_names()`, generated by the same macro that
/// generates the `match` arms, so the Rust side cannot drift.
///
/// The trade is that argument deserialization moves from the Tauri macro to
/// us, camelCase mapping included — covered by `core::dispatch`'s round-trip
/// test over every command.
#[tauri::command]
async fn rpc(
    state: tauri::State<'_, AppState>,
    command: String,
    args: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let ctx = state.clone_ctx();

    // Only `lcu_status`, `backfill_match_metadata` and `champion_icon` await
    // anything; every other command is blocking work — SQLite, a directory
    // scan, ffmpeg — and running it on an async worker would occupy that
    // worker for the duration.
    // Tauri used to make this choice per command by whether it was declared
    // `async`; now the dispatch table carries it.
    if core::is_async_command(&command) {
        return core::dispatch(&ctx, &command, args).await;
    }
    tauri::async_runtime::spawn_blocking(move || core::dispatch_blocking(&ctx, &command, args))
        .await
        .map_err(|e| e.to_string())?
}

/// Reveals the recordings folder in Finder/Explorer. Deliberately **not** in
/// the dispatch table: it drives the desktop shell, so it stays in the UI
/// process when the recorder moves into its own — an Explorer window launched
/// from a background daemon can open behind the foreground app.
///
/// Also done here rather than from the frontend with
/// `@tauri-apps/plugin-opener`: the JS `openPath` command is gated on the
/// opener scope, which is empty, so it always denies. The Rust function is a
/// plain call with no ACL involved. The folder is only created on the first
/// recording, so create it first — `open_path` stats the path and fails on a
/// fresh install otherwise.
#[tauri::command]
fn open_recordings_folder(state: tauri::State<AppState>) -> Result<(), String> {
    std::fs::create_dir_all(&state.recordings_dir).map_err(|e| e.to_string())?;
    tauri_plugin_opener::open_path(&state.recordings_dir, None::<&str>).map_err(|e| e.to_string())
}


/// `core::Autostart` over `tauri-plugin-autostart`.
///
/// Lives here rather than in `core` because `autolaunch()` hangs off an
/// `AppHandle`, and `core` may not name one (see its header). The manager is
/// resolved per call rather than cached: it is a cheap state lookup, and the
/// plugin owns the lifetime.
struct PluginAutostart(tauri::AppHandle);

impl core::Autostart for PluginAutostart {
    fn is_enabled(&self) -> Result<bool, String> {
        use tauri_plugin_autostart::ManagerExt;
        self.0.autolaunch().is_enabled().map_err(|e| e.to_string())
    }

    fn enable(&self) -> Result<(), String> {
        use tauri_plugin_autostart::ManagerExt;
        self.0.autolaunch().enable().map_err(|e| e.to_string())
    }

    fn disable(&self) -> Result<(), String> {
        use tauri_plugin_autostart::ManagerExt;
        self.0.autolaunch().disable().map_err(|e| e.to_string())
    }
}

// ---------------------------------------------------------------- updates
//
// The network half of `crate::update`, which owns the decision half and its
// tests. Everything here needs an `AppHandle`, so none of it can live in
// `core` (see that module's header).

/// Records an update result and tells the frontend to re-read it.
///
/// The two go together every time — a stored result nothing is told about is
/// a status the About block shows six hours late.
///
/// `CheckResult::Failed` carries a **whole sentence**, because the frontend
/// prints it verbatim. That is what lets a failed *install* and a failed
/// *check* share one state without the UI having to guess which it is
/// looking at.
fn record_update_result(app: &tauri::AppHandle, found: update::CheckResult) {
    use tauri::Emitter;
    match &found {
        update::CheckResult::Found(offer) => {
            info!("update", "{} is available", offer.version)
        }
        update::CheckResult::Failed(e) => warn!("update", "{e}"),
        _ => debug!("update", "no update available"),
    }
    app.state::<AppState>().set_update_check_result(found);
    if let Err(e) = app.emit(UPDATE_STATUS_EVENT, ()) {
        warn!("update", "failed to emit update-status-changed: {e}");
    }
}

/// Builds an updater pointed at the channel this install follows.
///
/// Stable uses the endpoints as configured in `tauri.conf.json`, so that URL
/// has one copy rather than two that can drift. Alpha overrides them at
/// runtime, which `UpdaterBuilder::endpoints` exists for.
///
/// A channel read that fails falls back to stable rather than propagating:
/// the conservative channel is the right answer to "we could not tell", and
/// the alternative is an install that stops checking because its preferences
/// table hiccuped.
fn updater_for_channel(app: &tauri::AppHandle) -> tauri_plugin_updater::Result<tauri_plugin_updater::Updater> {
    use tauri_plugin_updater::UpdaterExt;

    let channel = {
        let state = app.state::<AppState>();
        let stored = state
            .db
            .get_ui_prefs()
            .ok()
            .and_then(|prefs| prefs.get(update::CHANNEL_PREF_KEY).cloned());
        update::Channel::from_pref(stored.as_deref())
    };

    match channel {
        update::Channel::Stable => app.updater(),
        update::Channel::Alpha => app
            .updater_builder()
            .endpoints(vec![tauri::Url::parse(update::ALPHA_ENDPOINT)
                .expect("ALPHA_ENDPOINT is a literal and is unit-tested as a URL")])?
            .build(),
    }
}

/// Runs one update check and records what it found.
///
/// Never returns a `Result`: nothing calls this that could act on one. A
/// failed check is a *state* the About block renders, not an error to
/// propagate — the user's network being down is not a bug.
async fn run_update_check(app: tauri::AppHandle) {
    let found = match updater_for_channel(&app) {
        Err(e) => update::CheckResult::Failed(format!("Could not check for updates: {e}")),
        Ok(updater) => match updater.check().await {
            Ok(Some(u)) => update::CheckResult::Found(update::UpdateOffer {
                version: u.version.clone(),
                notes: u.body.clone(),
                pub_date: u.date.map(|d| d.to_string()),
            }),
            Ok(None) => update::CheckResult::NothingNewer,
            // Includes the ordinary "this platform has no entry in
            // `latest.json`", which is what any build made off Windows
            // gets: Windows is the only platform that ships
            // (DEVELOPMENT.md §14).
            Err(e) => update::CheckResult::Failed(format!("Could not check for updates: {e}")),
        },
    };

    record_update_result(&app, found);
}

/// Downloads the offered installer and hands the machine over to it.
///
/// **This ends the process**, one way or another: on Windows the plugin spawns
/// the NSIS installer and exits, and the `app.exit(0)` below is the fallback
/// for a platform where it returns instead.
///
/// `core::install_update` has already checked that nothing is being recorded.
/// The finalize here is the belt for the gap between that check and this
/// moment — the same reasoning, and the same call, as `tray::request_quit`.
async fn run_update_install(app: tauri::AppHandle) {
    let prep_app = app.clone();
    let finalized = tauri::async_runtime::spawn_blocking(move || {
        let (supervisor, recorder) = {
            let state = prep_app.state::<AppState>();
            (
                Arc::clone(&state.supervisor),
                Arc::clone(&state.recorder),
            )
        };
        let finalized = supervisor.finalize_for_shutdown();

        // **The installer cannot overwrite a file another process has open,
        // and the capture backend is another process.** libobs runs
        // out-of-process (`extprocess_recorder.exe`, DEVELOPMENT.md §2.2),
        // it comes up as soon as the League client appears, and it holds
        // every DLL in the bundled `libobs/` resource folder open while it
        // lives. NSIS then fails on the first one it tries to replace with
        // "Error opening file for writing: …\libobs\avcodec-61.dll" and an
        // Abort/Retry/Ignore box — which is the *good* outcome; Ignore would
        // leave a new worker beside an old DLL.
        //
        // NSIS's own "close the running app" check cannot help: it keys off
        // `mainBinaryName`, and the worker is a different executable it has
        // never heard of.
        //
        // `release` is a no-op while recording, which is why the gate above
        // has already established that nothing is.
        match recorder.lock() {
            Ok(mut backend) => backend.release(),
            // Not fatal: the install may still succeed if the worker was
            // never up. Worth a line, because if it *was* up this is the
            // reason the installer is about to complain.
            Err(e) => warn!("update", "could not release the capture backend: {e}"),
        }
        finalized
    })
    .await;
    if let Ok(true) = finalized {
        info!("update", "finalized an in-flight recording before updating");
    }

    // Killing the worker is asynchronous on Windows: the IPC link's `Drop`
    // asks it to go, and the handles it holds are released when the process
    // actually exits, not when we stop waiting. The installer runs moments
    // from now, so give it a beat.
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    // Re-checked rather than cached from the background poll: `Update` owns
    // the download URL and its signature, and holding one for up to six hours
    // across a release means installing something the endpoint has since
    // moved on from.
    // Every path out of here that is *not* a successful install records a
    // result and emits. The frontend put the row into "Downloading…" the
    // moment the button was pressed, and it has no other way to learn that
    // this did not happen — `install_update` returned the instant the request
    // was handed over, long before any of this ran.
    let offer = match updater_for_channel(&app) {
        Ok(updater) => match updater.check().await {
            Ok(Some(u)) => u,
            Ok(None) => {
                record_update_result(&app, update::CheckResult::NothingNewer);
                return;
            }
            Err(e) => {
                record_update_result(
                    &app,
                    update::CheckResult::Failed(format!(
                        "Could not install the update: {e}"
                    )),
                );
                return;
            }
        },
        Err(e) => {
            record_update_result(
                &app,
                update::CheckResult::Failed(format!("Could not install the update: {e}")),
            );
            return;
        }
    };

    info!("update", "installing {}", offer.version);
    if let Err(e) = offer.download_and_install(|_, _| {}, || {}).await {
        // Left in whatever state it reached, but still running and still
        // recording-capable — nothing here has touched the installed app yet.
        record_update_result(
            &app,
            update::CheckResult::Failed(format!("Could not install the update: {e}")),
        );
        return;
    }
    app.exit(0);
}

/// Whether this build is allowed to update itself.
///
/// A **runtime** `cfg!` rather than a `#[cfg]` around the callers,
/// deliberately. A devtools bundle must never update itself — it would replace
/// itself with the production app, and `tauri.devtools.conf.json` renames the
/// product precisely so the two can coexist — but compiling the update wiring
/// out under `--features devtools` would leave `CheckResult`'s variants and
/// `Ctx`'s two update setters constructed by nothing, which is dead code that
/// `-D warnings` fails the devtools clippy run over (CLAUDE.md). This way both
/// configurations compile the same code and only the behaviour differs.
fn updates_enabled() -> bool {
    !cfg!(feature = "devtools")
}

/// Wires the update seam onto `Ctx`. Must run *before* the `Ctx` is handed to
/// `manage`, which is what takes the `&mut`.
fn wire_updates(app: &tauri::AppHandle, ctx: &mut core::Ctx) {
    if !updates_enabled() {
        info!("update", "devtools build: updates are off");
        // Said explicitly rather than left to the default. `Ctx::new` seeds
        // `Pending` — "checking…" — because a production build has not
        // checked yet at this point either, and reporting "not available in
        // this build" for the first thirty seconds after every launch is the
        // most alarming possible wording for "hang on".
        ctx.set_update_check_result(update::CheckResult::Unsupported);
        return;
    }

    let request_handle = app.clone();
    ctx.set_update_requester(Box::new(move |request| {
        let app = request_handle.clone();
        match request {
            update::UpdateRequest::Check => {
                tauri::async_runtime::spawn(run_update_check(app));
            }
            update::UpdateRequest::Install => {
                tauri::async_runtime::spawn(run_update_install(app));
            }
        }
    }));
}

/// Starts the six-hourly check.
///
/// Called *after* `manage`, not with `wire_updates`: `run_update_check` reads
/// `AppState` back off the handle, and a task spawned before the state exists
/// would be relying on its own start-up delay to paper over the ordering.
fn spawn_update_poll(app: &tauri::AppHandle) {
    if !updates_enabled() {
        return;
    }
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(UPDATE_FIRST_CHECK_DELAY).await;
        loop {
            run_update_check(handle.clone()).await;
            tokio::time::sleep(UPDATE_CHECK_INTERVAL).await;
        }
    });
}

/// The main window's label. Matches `capabilities/default.json`'s
/// `"windows": ["main"]`, which is what Tauri would have used implicitly when
/// the window came from `tauri.conf.json`.
pub(crate) const MAIN_WINDOW_LABEL: &str = "main";

/// Builds the main window.
///
/// It used to come from `app.windows` in `tauri.conf.json`, which Tauri
/// creates automatically *before* `setup` runs — so there was no way to not
/// have one. A hidden start needs exactly that: `visible: false` still
/// constructs the WebView2 instance and pays its full cost, which defeats the
/// purpose of starting in the tray (DEVELOPMENT.md §12).
///
/// Safe to call from `setup`, unlike `dev_open_portal`, which documents why it
/// must be `async`: that hazard is building a window re-entrantly from inside
/// a WebView2 IPC callback, and `setup` is not one. Any window created later
/// from a tray click or an IPC notification does have to worry about it.
///
/// `view` asks the frontend to open on a particular view. It rides in on the
/// URL fragment rather than an event because a window that has only just been
/// created is not listening yet — the tray's "Settings" item opens a cold
/// window and still has to land on the settings page.
pub(crate) fn create_main_window(app: &tauri::AppHandle, view: Option<&str>) -> tauri::Result<()> {
    let url = match view {
        Some(view) => format!("index.html#{view}"),
        None => "index.html".to_string(),
    };
    tauri::WebviewWindowBuilder::new(app, MAIN_WINDOW_LABEL, tauri::WebviewUrl::App(url.into()))
        .title("ninja-recorder")
        // Sized around the frontend's own `--content-max: 1120px` plus the
        // container's padding and room for a scrollbar, so the default
        // window is exactly wide enough for the content column to reach its
        // full width and stop — one pixel narrower and every view squeezes,
        // wider and the column just centres itself in more background.
        //
        // The height clears the review view's player and its timeline; the
        // marker list below them is deliberately left to scroll rather than
        // opening a window taller than a 1080p desktop can show.
        .inner_size(1200.0, 900.0)
        .min_inner_size(960.0, 640.0)
        .build()?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let mode = launch::Launch::from_env();
    if let Some(why) = mode.unsupported() {
        // Deliberately not through the log facade: this runs before
        // `setup`, so before `log::init`, and exits immediately. There is
        // no file to write to yet.
        eprintln!("[launch] {why}");
        std::process::exit(2);
    }

    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        // The argument list is the on-disk contract: it is written into
        // `HKCU\...\Run` once, when the user ticks the box, and handed
        // back to whatever build is installed years later — which is why
        // `launch.rs` owns the string and pins it with a test. `--hidden`
        // and not `--daemon`: the daemon is still reserved, and registering
        // a flag that exits 2 would mean a login start that records nothing.
        //
        // Nothing is registered by installing; the entry only appears when
        // the settings toggle is turned on.
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec![launch::HIDDEN_FLAG]),
        ))
        // Registered unconditionally; whether it is ever *used* is
        // `updates_enabled`. The endpoint and the public key that verifies
        // what it serves live in `tauri.conf.json` under `plugins.updater`.
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(move |app| {
            // First thing in setup, and deliberately before the recorder
            // backend and the database: a release build has no console
            // (`main.rs`), so until this runs, anything that goes wrong
            // goes nowhere. Failing to open the library is one of the
            // failures most worth having a record of.
            match app.path().app_data_dir() {
                Ok(data_dir) => match log::init(&data_dir.join("logs")) {
                    Some(path) => info!("log", "logging to {}", path.display()),
                    // Only reachable via stderr, which in a release build
                    // is nowhere — but in `tauri:dev` it is exactly where
                    // someone would be looking.
                    None => eprintln!("[log] could not open a log file; this session logs to stderr only"),
                },
                Err(e) => eprintln!("[log] no app data directory, so no log file: {e}"),
            }

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
                            error!(
                                "recorder",
                                "could not locate the libobs worker, recording disabled: {e}"
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
            info!("recorder", "backend: {}", backend.backend_name());

            let recorder: Arc<Mutex<Box<dyn Recorder>>> = Arc::new(Mutex::new(backend));
            let dir = recordings_dir(app.handle())?;

            // Must happen before the supervisor starts polling — see
            // fixtures::set_base_dir's doc comment.
            fixtures::set_base_dir(app.path().app_data_dir()?.join("fixtures"));
            fixtures::init_from_env();
            // Worth a line: capture is on by default until v1.0 and writes
            // a file per response, so a user should be able to find out
            // that it is happening and where it is going without reading
            // the source (DEVELOPMENT.md §3.3).
            if fixtures::enabled() {
                info!(
                    "fixtures",
                    "capturing API responses to {}",
                    app.path().app_data_dir()?.join("fixtures").display()
                );
            }

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
                    error!("db", "cannot open the VOD library at {}: {e}", db_path.display());
                    // Kept as a console write on top of the log line: this
                    // is a wall of actionable prose aimed at a person in a
                    // terminal, not a log entry.
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
                    error!("db", "cannot open the VOD library at {}: {e}", db_path.display());
                    std::process::exit(1);
                }
            });

            // Bound to a local rather than inlined: the probe borrows from
            // it for the length of the call.
            let reconcile_ffmpeg = ffmpeg_path(app.handle());
            match db::reconcile::reconcile(&db, &dir, reconcile_ffmpeg.as_deref()) {
                Ok(report) if report.orphans_removed > 0 || report.imported > 0 => {
                    info!(
                        "db",
                        "startup reconcile: removed {} orphan row(s), imported {} untracked file(s)",
                        report.orphans_removed, report.imported
                    );
                }
                Ok(_) => {}
                Err(e) => error!("db", "startup reconcile failed: {e}"),
            }

            // Retention (DEVELOPMENT.md §6): enforced here and
            // again after every finalize (state_machine::supervisor), so a
            // policy set while the app was closed — or last session's
            // finalize enforcement never running because the app crashed
            // — still gets applied on the next launch.
            match db.get_retention_policy() {
                Ok(policy) => match retention::enforce_now(&db, &policy) {
                    Ok(report) if !report.deleted.is_empty() => info!(
                        "retention",
                        "startup enforcement: removed {} recording(s), freed {} bytes",
                        report.deleted.len(),
                        report.freed_bytes
                    ),
                    Ok(_) => {}
                    Err(e) => error!("retention", "startup enforcement failed: {e}"),
                },
                Err(e) => error!("retention", "failed to load policy: {e}"),
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
            supervisor.set_event_notifier(Box::new(move |event| {
                use state_machine::SupervisorEvent;
                use tauri::{Emitter, Manager};

                // Anything the frontend needs to react to still goes out as a
                // Tauri event; the notifications are extra, and only reach the
                // user when they have no window open to look at.
                let notify_for = |kind: core::NotifyKind, title: &str, body: &str| {
                    let ctx = notify_handle.state::<AppState>().clone_ctx();
                    notify::notify(&notify_handle, &ctx, kind, title, body);
                };

                match event {
                    SupervisorEvent::LibraryChanged => {
                        if let Err(e) = notify_handle.emit(LIBRARY_CHANGED_EVENT, ()) {
                            warn!("state_machine", "failed to emit library-changed: {e}");
                        }
                    }
                    SupervisorEvent::RecordingStarted => notify_for(
                        core::NotifyKind::RecordingStarted,
                        "Recording started",
                        "ninja-recorder is capturing this game.",
                    ),
                    SupervisorEvent::Finalized(finalized) => {
                        if let Err(e) = notify_handle.emit(LIBRARY_CHANGED_EVENT, ()) {
                            warn!("state_machine", "failed to emit library-changed: {e}");
                        }
                        let name = std::path::Path::new(&finalized.path)
                            .file_stem()
                            .map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_else(|| finalized.path.clone());
                        let markers = finalized.markers.len();
                        // No champion or KDA here: those columns are still
                        // NULL on real recordings (DEVELOPMENT.md §3.4), so
                        // the toast says what is actually known.
                        let body = if markers == 1 {
                            format!("{name} — 1 marker")
                        } else {
                            format!("{name} — {markers} markers")
                        };
                        notify_for(
                            core::NotifyKind::RecordingFinished,
                            "Recording saved",
                            &body,
                        );
                    }
                    SupervisorEvent::RecordingFailed(message) => notify_for(
                        core::NotifyKind::RecordingFailed,
                        "Recording problem",
                        &message,
                    ),
                }
            }));
            // The other half of the finalize: `stop_recording` writes the
            // row from what Live Client Data established, then hands the
            // game's identifiers here so the LCU's post-game columns can
            // be filled in once the client actually has them. Installed
            // from `run()` for the same reason the notifier above is —
            // this is where the async runtime is allowed to be reachable
            // from.
            // The loading screen comes off the file itself, on a blocking
            // thread rather than inline: it is a stream copy of something
            // that can be gigabytes, and `stop_recording` calls this under
            // the recorder lock — where holding on would block the header's
            // 1 Hz `is_recording` poll and the start of the next game.
            //
            // A build with no ffmpeg installs nothing and keeps the whole
            // file, which is what every recording did before this existed.
            if let Some(ffmpeg) = ffmpeg_path(app.handle()) {
                let trim_db = Arc::clone(&db);
                let trim_handle = app.handle().clone();
                supervisor.set_trim_requester(Box::new(move |recording_id| {
                    let db = Arc::clone(&trim_db);
                    let ffmpeg = ffmpeg.clone();
                    let handle = trim_handle.clone();
                    tauri::async_runtime::spawn_blocking(move || {
                        use tauri::Emitter;
                        match trim::trim_recording(&db, &ffmpeg, recording_id) {
                            Ok(report) => {
                                // Both ends named separately: only the head
                                // shifts markers, so if a rebase ever looks
                                // wrong this line says which number to blame.
                                info!("trim",
                                    "cut {:.1}s off recording {recording_id} \
                                     ({:.1}s loading screen, {:.1}s post-game)",
                                    report.removed_s,
                                    report.head_removed_s,
                                    report.tail_removed_s
                                );
                                // The card's length and size both changed.
                                if let Err(e) = handle.emit(LIBRARY_CHANGED_EVENT, ()) {
                                    warn!("trim", "failed to emit library-changed: {e}");
                                }
                            }
                            // Includes the ordinary "nothing to cut", which
                            // is what a reconnect and a client that reported
                            // the game late both look like.
                            Err(e) => {
                                debug!("trim", "no trim for recording {recording_id}: {e}")
                            }
                        }
                    });
                }));
            }

            let summary_db = Arc::clone(&db);
            let summary_handle = app.handle().clone();
            // Finishes patches an app exit interrupted, once a client is
            // reachable again (#137). The gold curve in particular is written
            // by the deferred patch and by nothing else, so without this a
            // quit — or an in-app update, which exits by design — inside its
            // one-minute window loses it for good.
            let resume_db = Arc::clone(&db);
            let resume_handle = app.handle().clone();
            supervisor.set_summary_resumer(Box::new(move |lockfile| {
                let db = Arc::clone(&resume_db);
                let handle = resume_handle.clone();
                tauri::async_runtime::spawn(async move {
                    use tauri::Emitter;
                    let now_ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as i64)
                        .unwrap_or(0);
                    if match_summary::resume_pending(&db, &lockfile, now_ms).await > 0 {
                        // Rows changed minutes or days after the library last
                        // looked at them.
                        if let Err(e) = handle.emit(LIBRARY_CHANGED_EVENT, ()) {
                            warn!("match-summary", "failed to emit library-changed: {e}");
                        }
                    }
                });
            }));

            supervisor.set_summary_fetcher(Box::new(move |request| {
                let db = Arc::clone(&summary_db);
                let handle = summary_handle.clone();
                tauri::async_runtime::spawn(async move {
                    use tauri::Emitter;
                    if match_summary::patch(&db, &request).await {
                        // Nothing else will tell the frontend: the row
                        // changed minutes after the library last refreshed.
                        if let Err(e) = handle.emit(LIBRARY_CHANGED_EVENT, ()) {
                            warn!("match-summary", "failed to emit library-changed: {e}");
                        }
                    }
                });
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
                app.path().app_data_dir()?.join("ddragon"),
                ffmpeg_path(app.handle()),
            );
            ctx.set_autostart(Box::new(PluginAutostart(app.handle().clone())));
            wire_updates(app.handle(), &mut ctx);
            let notify_handle = app.handle().clone();
            ctx.set_library_changed_notifier(Box::new(move || {
                use tauri::Emitter;
                if let Err(e) = notify_handle.emit(LIBRARY_CHANGED_EVENT, ()) {
                    warn!("core", "failed to emit library-changed: {e}");
                }
            }));

            app.manage(AppState(Arc::new(ctx)));
            #[cfg(feature = "devtools")]
            app.manage(dev::DevState::default());

            // After `manage`, because the check reads `AppState` back off the
            // handle to record what it found.
            spawn_update_poll(app.handle());

            // Before the window: the tray is what makes a `--hidden` start
            // reachable at all, so it must exist even if window creation
            // fails.
            if let Err(e) = tray::build(app.handle()) {
                error!("tray", "could not create the tray icon: {e}");
            }

            // Last, so the window never renders against half-built state:
            // the frontend starts polling as soon as it loads.
            if mode.creates_window() {
                create_main_window(app.handle(), None)?;
            } else {
                info!("launch", "started in the tray with no window");
            }
            Ok(())
        });

    // `generate_handler!` takes a literal path list — it can't host a
    // `#[cfg]` attribute or a macro expansion inside the brackets — so the
    // production and devtools surfaces still need two spellings. They are
    // now two items and thirty rather than twenty-three and fifty-one:
    // everything except the shell-driving commands goes through `rpc`, and
    // `core::command_names()` is the one list of what that reaches.
    #[cfg(not(feature = "devtools"))]
    let builder = builder.invoke_handler(tauri::generate_handler![rpc, open_recordings_folder]);

    #[cfg(feature = "devtools")]
    let builder = builder.invoke_handler(tauri::generate_handler![
        rpc,
        open_recordings_folder,
        dev::dev_open_portal,
        dev::dev_env_info,
        dev::dev_health,
        dev::dev_registered_commands,
        dev::dev_open_data_dir,
        dev::dev_log_files,
        dev::dev_read_log,
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
        dev::dev_champion_name,
        dev::dev_fetch_match_summary,
        dev::dev_patch_match_summary,
        dev::dev_live_client_probe,
        dev::dev_fixtures_state,
        dev::dev_shape_report,
        dev::dev_recording_report,
        dev::dev_recording_vs_lcu,
        dev::dev_fixture_read,
        dev::dev_fixture_write,
        dev::dev_set_fixture_recording,
        dev::dev_trim_lead_in,
    ]);

    let builder = builder.on_window_event(|window, event| {
        let tauri::WindowEvent::CloseRequested { api, .. } = event else {
            return;
        };
        if window.label() != MAIN_WINDOW_LABEL {
            return;
        }

        let ctx = window.state::<AppState>().clone_ctx();
        let action = core::close_action(&ctx);

        // Both surviving actions leave the app running with no window, which
        // is exactly when it looks like it has quit. Say so once.
        if !matches!(action, core::CloseAction::Quit) {
            notify::close_to_tray_notice(&window.app_handle().clone(), &ctx);
        }

        match action {
            // Let the window be destroyed. The process survives because
            // `ExitRequested` is vetoed below, and destroying the webview is
            // what actually reclaims its memory — hiding reclaims nothing.
            core::CloseAction::CloseWindow => {}
            core::CloseAction::Hide => {
                api.prevent_close();
                let _ = window.hide();
            }
            // Route through the tray's own quit so an in-flight recording is
            // finalized rather than dropped.
            core::CloseAction::Quit => {
                api.prevent_close();
                tray::request_quit(&window.app_handle().clone());
            }
        }
    });

    builder
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app, event| {
            // `code: None` means the exit came from user interaction — here,
            // the last window closing. That must not end the process: the
            // recorder keeps running in the tray, which is the entire point.
            // An explicit `AppHandle::exit` arrives as `Some(_)` and is
            // allowed through, which is how the tray's Quit gets out.
            if let tauri::RunEvent::ExitRequested { code: None, api, .. } = event {
                api.prevent_exit();
            }
        });
}
