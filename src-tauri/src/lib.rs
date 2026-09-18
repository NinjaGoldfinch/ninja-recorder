mod audio_tracks;
mod backfill;
mod contract;
/// The TypeScript emitter, re-exported for the `gen-contract` binary.
///
/// A single named re-export rather than making `contract` public: the binary
/// needs exactly this, and the rest of the module is internal to the crate.
pub use contract::r#gen as gen_contract;
mod core;
// `pub` because `main.rs` dispatches on it before either side is built, and
// the daemon's refusal is a value it returns rather than a string in
// `launch.rs` — see `daemon::run`.
pub mod daemon;
mod db;
mod ddragon;
#[cfg(feature = "devtools")]
mod dev;
mod fixtures;
// `pub` for the same reason as `daemon`: `main.rs` reads the mode.
pub mod launch;
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
mod ui;
mod update;

// No `use crate::{error, warn, info}` here, unlike every other module:
// this file *is* the crate root, and `#[macro_export]` already puts the
// macros in its macro namespace. Importing them would collide with the
// definitions themselves (E0255).
use recorder::{FailedRecorder, Recorder};
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

/// The contract's event channel — WS2.3.
///
/// One Tauri event carrying every `contract::events::Event`, because the
/// contract's own discriminant is `type` and a client switches on that. The two
/// constants above are the v1 shape: a separate channel per signal, carrying
/// `()`, with the payload fetched afterwards by command. WS2.6's generated
/// client subscribes here instead, and WS2.7 deletes them.
///
/// Named `event` because that is what the plan's transport interface listens
/// for (§4.1).
pub(crate) const CONTRACT_EVENT: &str = "event";

/// Raised when the close button means "quit everything", so the frontend can
/// run that flow.
///
/// The decision is Rust's, because only this side sees the native close. What
/// happens next is the frontend's, because it owns the one thing this process
/// cannot do well from here: ask the person, in the window they just clicked,
/// with a dialog that belongs to it. The daemon's own confirmation is a
/// `MessageBoxW` with no owner window, which is right for a tray click and
/// would appear behind the app for this one.
pub(crate) const QUIT_REQUESTED_EVENT: &str = "quit-requested";

// The check's schedule moved to `daemon::update` with the check (WS3.6). The
// two constants that named it went with it rather than being left here for a
// caller that no longer exists.

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
pub(crate) fn which_ffmpeg() -> Option<std::path::PathBuf> {
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
    link: tauri::State<'_, ui::link::DaemonLink>,
    command: String,
    args: serde_json::Value,
) -> Result<serde_json::Value, String> {
    // Forwarded, not dispatched. This used to run the command in this process
    // against this process's `Ctx`; since WS3.4 the command runs in the daemon,
    // which is the process that owns the supervisor, the recorder and every
    // write. The name and the JSON are unchanged, which is what lets every view
    // keep working across the move.
    //
    // `is_async_command` and `spawn_blocking` went with it: whether a command
    // blocks is a question for the process that runs it, and `daemon::rpc`
    // answers it there.
    link.call(&command, args).await
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
/// Ends this process, and only this one.
///
/// The recorder is stopped separately, by `quit_recorder` over the pipe, and
/// the frontend calls them in that order. Split in two because they are two
/// processes and the answer for each is different: the window always goes, the
/// recorder only goes if the person said so.
///
/// No finalize here, deliberately. `tray::request_quit` used to run one
/// through this process's supervisor, which since WS3.4 is never started and
/// holds a `FailedRecorder`, so it protected nothing while reading as though
/// it did. What finalizes a recording is the daemon, on its own way out.
#[tauri::command]
fn exit_ui(app: tauri::AppHandle) {
    info!("launch", "the window process is exiting; the recorder was handled separately");
    // `exit` rather than closing the last window: `RunEvent::ExitRequested`
    // vetoes `code: None`, which is what keeps a closed window from ending the
    // process, and an explicit exit arrives as `Some(_)` and is let through.
    app.exit(0);
}

#[tauri::command]
fn open_recordings_folder(state: tauri::State<AppState>) -> Result<(), String> {
    std::fs::create_dir_all(&state.recordings_dir).map_err(|e| e.to_string())?;
    tauri_plugin_opener::open_path(&state.recordings_dir, None::<&str>).map_err(|e| e.to_string())
}


// **No `Autostart` here since #151.** `PluginAutostart` lived at this point
// and wrapped `tauri-plugin-autostart`, which needs an `AppHandle`. It was
// setting the seam on *this* process's `Ctx`, and WS3.4 moved the commands
// that read it into the daemon, so it had been answering nobody for several
// workstreams while the settings row said the feature was unavailable.
// `daemon::autostart` owns it now, over the crate that plugin wraps.

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
    // The one event in the table the supervisor cannot emit: update status
    // lives in `AppState`, not in the state machine, so it is published from
    // the place that changes it rather than through the sink.
    // Built through the same `get_update_status` the command uses, so the
    // pushed value and the polled one cannot disagree — the installability half
    // is recomputed per read, and a status decided any other way here would be
    // a second opinion about the same question.
    match core::get_update_status(&app.state::<AppState>().clone_ctx()) {
        Ok(status) => {
            if let Err(e) = app.emit(
                CONTRACT_EVENT,
                &contract::events::Event::UpdateStatus { status },
            ) {
                warn!("update", "failed to emit the update-status contract event: {e}");
            }
        }
        // The `update-status-changed` ping above has already gone out, so a
        // client still learns to re-read. Nothing is lost but the payload.
        Err(e) => warn!("update", "could not build the update-status event: {e}"),
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
pub(crate) fn updates_enabled() -> bool {
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

// The six-hourly check moved to `daemon::update` (WS3.6). It used to run here
// and record what it found in this process's `Ctx`, which stopped being read
// the moment WS3.4 forwarded every command to the daemon: the About block asks
// `get_update_status`, that goes over the pipe, and the daemon answered from a
// cell nothing had ever filled. So the status was "Checking" forever while a
// perfectly good check ran in the wrong process.
//
// The *install* half is still here and still unreachable for the same reason.
// It is the rest of WS3.6, and `daemon::update`'s header says what it becomes.

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
    // Only ever `Ui` or `UiHidden`: `main.rs` matches on the mode first and
    // sends `Daemon` to `daemon::run`, which is the whole point of splitting
    // the dispatch out of here. Read again rather than passed in so that the
    // mobile entry point below still has one argument-free way in.
    let mode = launch::Launch::from_env();

    // **Before the builder, not inside `setup`.** This is the correction the
    // daemon already got (`daemon::start`): open the log before anything that
    // can fail, because a process that dies earlier than its first log line
    // leaves no trace anywhere.
    //
    // The UI never got it, and it is the process with the most that can fail
    // before `setup` runs — four plugin initialisations, the context, the
    // whole of `build()` — every one of them reporting through a stderr that a
    // `windows_subsystem = "windows"` build sends nowhere. The symptom is an
    // app that flashes and closes with `app_data_dir()` not existing at all,
    // which is exactly the state where the only evidence would have been in a
    // file that was never opened.
    //
    // `Paths::resolve` rather than `app.path()`, because there is no `app`
    // yet. It is the same `dirs::data_dir()` join that Tauri's own
    // `app_data_dir()` performs — see its header for why that is one rule
    // rather than two implementations of one.
    match daemon::Paths::resolve() {
        Ok(paths) => match log::init(&paths.logs, log::Process::Ui) {
            Some(path) => info!("log", "logging to {}", path.display()),
            // Only reachable via stderr, which in a release build is nowhere
            // — but in `tauri:dev` it is exactly where someone would look.
            None => {
                eprintln!("[log] could not open a log file; this session logs to stderr only")
            }
        },
        Err(e) => eprintln!("[log] no app data directory, so no log file: {e}"),
    }
    info!("launch", "the window process is starting ({mode:?})");

    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        // **No autostart plugin since #151.** It was registered here so that
        // `PluginAutostart` could reach `autolaunch()`, and that seam had been
        // answering nobody since WS3.4 moved the commands into the daemon.
        // `daemon::autostart` writes the entry now, over the crate this plugin
        // wraps, and `launch::autostart_args` is still the one place the flag
        // is chosen.
        // Registered unconditionally; whether it is ever *used* is
        // `updates_enabled`. The endpoint and the public key that verifies
        // what it serves live in `tauri.conf.json` under `plugins.updater`.
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(move |app| {
            // The log is already open — `run` does it above, before the
            // builder, so that everything between there and here is recorded
            // too. `app_data_dir()` below resolves to the same directory it
            // was opened under, which `Paths::resolve` is written to
            // guarantee and `daemon`'s identifier test pins.

            // **The UI links no capture backend.** It used to build the real
            // one here, which is what made killing the window kill the
            // recording. The daemon owns the `Recorder` now (§3.1's ownership
            // table).
            //
            // `FailedRecorder` rather than `StubRecorder`, and rather than
            // restructuring `Ctx` to make the backend optional. It refuses
            // every call with the reason, which is what this process should do:
            // the stub *fabricates* a recording by copying a fixture, so
            // anything that reached for it here would quietly produce a file
            // rather than say it was in the wrong process. `backend_name()`
            // reports the message, so the portal's Overview shows it too.
            //
            // It is also the type that compiles everywhere: `stub` is gated to
            // non-Windows plus `cfg(test)`, which CI caught and this box could
            // not.
            let backend: Box<dyn Recorder> = Box::new(FailedRecorder(
                "this process does not record; the daemon does".to_string(),
            ));

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
            // **Read-only, and SQLite is what enforces it** (§4.4). Every
            // command that could write is forwarded to the daemon, and the
            // startup reconcile and retention pass went with it; what still
            // reads this connection is the portal's UI-side commands and,
            // in time, the library grid.
            //
            // `open_read_only` runs no migrations. The daemon owns those, and
            // a second process running them would be both a write and a race.
            // Opening before the daemon has created the file yields a
            // connection that sees no tables, which resolves itself the moment
            // the daemon migrates.
            let db = Arc::new(match db::Db::open_read_only(&db_path) {
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

            // **The startup reconcile and the retention pass are the daemon's.**
            // Both write, and every write belongs to the process that owns the
            // writer connection (§3.1). They used to run here, and running them
            // in both processes would mean two folder scans racing to import
            // the same untracked file and two retention passes each deciding
            // what to delete from a list the other was deleting from.
            //
            // `daemon::serve` runs them in the same order, at the same point in
            // startup, and publishes what they found.

            // **Nothing here drives the supervisor any more.** It used to be
            // built, wired to five callbacks and started, which is what made
            // this process the recorder. The daemon does all of that since
            // WS3.2 (`daemon::run`), including the contract event sink, the
            // notifications, the trim and the deferred match-summary patch.
            //
            // A supervisor is still constructed because `Ctx` holds one and the
            // portal's UI-side commands read through it, but `start()` is never
            // called: no lockfile watch, no gameflow watch, no Live Client
            // poll, and so no second state machine racing the daemon's for the
            // same game.
            let supervisor =
                state_machine::Supervisor::new(Arc::clone(&recorder), dir.clone(), Arc::clone(&db));

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
            wire_updates(app.handle(), &mut ctx);
            let notify_handle = app.handle().clone();
            ctx.set_library_changed_notifier(Box::new(move || {
                use tauri::Emitter;
                if let Err(e) = notify_handle.emit(LIBRARY_CHANGED_EVENT, ()) {
                    warn!("core", "failed to emit library-changed: {e}");
                }
            }));

            app.manage(AppState(Arc::new(ctx)));

            // The link to the daemon, and with it everything this process used
            // to do for itself. Starts connecting immediately and starts a
            // daemon if none answers; `rpc` below is a forward across it.
            match daemon::Paths::resolve() {
                Ok(paths) => ui::link::attach(app.handle(), daemon::rpc::endpoint(&paths.data)),
                // Nothing works without it: every command the frontend makes
                // goes over this. Said once, loudly, rather than as a failure
                // per call.
                Err(e) => error!("ui", "cannot work out where the daemon listens: {e}"),
            }

            // **No tray here.** The daemon owns it (§3.1, WS3.3), and this
            // process building a second one meant two identical icons in the
            // notification area whenever both were running, which since WS3.4
            // is whenever the window is open at all. What reached the tray from
            // this side — showing the window, and the close button's Quit —
            // is still in `tray.rs`; what is gone is the icon.

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
    let builder = builder.invoke_handler(tauri::generate_handler![
        rpc,
        ui::link::rpc_call,
        ui::link::rpc_subscribe,
        ui::link::rpc_health,
        open_recordings_folder,
        exit_ui
    ]);

    // The list lives in `contract::portal`, and this is the callback that turns
    // it into a `generate_handler!` invocation. `generate_handler!` cannot host
    // a macro expansion inside its brackets, but it can be the *output* of one,
    // which is what lets the registration and the portal's manifest come from
    // the same tokens instead of two hand-written lists (#74, WS2.7).
    //
    // Only the *UI* table since WS3.7. The other forty `dev_*` commands are
    // reached by name through `rpc`, the same way every production command is,
    // because they need the process that owns the database and the supervisor.
    // What is left here is the handful that need a window or the desktop shell,
    // and `dev_ui_command_table!`'s header says why for each one.
    #[cfg(feature = "devtools")]
    let builder = {
        macro_rules! with_dev_commands {
            ($( $(#[doc = $doc:literal])* $name:ident { $($body:tt)* } )*) => {
                tauri::generate_handler![
                    rpc,
                    ui::link::rpc_call,
                    ui::link::rpc_subscribe,
                    ui::link::rpc_health,
                    open_recordings_folder,
                    exit_ui,
                    $( dev::$name, )*
                ]
            };
        }
        builder.invoke_handler(dev_ui_command_table!(with_dev_commands))
    };

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
            // **Handed to the frontend, which owns the rest of it.** This
            // used to call `tray::request_quit`, which finalized through a
            // supervisor that has not been started since WS3.4 and then exited
            // this process, leaving the daemon recording with its tray icon
            // still sitting there. Quit closed the window and the app went on
            // running, which is the opposite of what the setting says (#136).
            //
            // What replaces it is two calls in the right order, made from the
            // frontend: stop the recorder over the pipe, ask the person first
            // if a game is in flight, then end this process.
            core::CloseAction::Quit => {
                api.prevent_close();
                use tauri::Emitter;
                if let Err(e) = window.emit(QUIT_REQUESTED_EVENT, ()) {
                    // Nothing is listening, so nothing will quit. Say so rather
                    // than leaving a close button that does nothing at all.
                    error!("launch", "could not ask the window to quit: {e}");
                }
            }
        }
    });

    builder
        .build(tauri::generate_context!())
        // `expect` here aborted with a message only stderr could carry, which
        // in a windowed build is nowhere. A plugin that failed to initialise,
        // a context that would not load and a `setup` that returned `Err` all
        // looked identical from outside: an app that closed as soon as it was
        // opened. The log is open by this point, so now they do not.
        //
        // Exit code 3, beside `daemon::run`'s 2 and the library's 1, so the
        // three fatal starts are still told apart by a caller that can only
        // see a number.
        .unwrap_or_else(|e| {
            error!("launch", "could not build the application: {e}");
            eprintln!("[launch] could not build the application: {e}");
            std::process::exit(3);
        })
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

