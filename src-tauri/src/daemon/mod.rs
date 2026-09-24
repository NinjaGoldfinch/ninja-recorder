//! The headless recorder daemon. WS3 task 3.2.
//!
//! One binary, two roles: `main.rs` reads argv and sends `--daemon` here. This
//! is what the UI's `lib.rs` setup does, minus everything that needs a window
//! — and it is the process that must outlive the UI, because killing a window
//! must not stop a recording or lose a marker (implementation plan §3.1).
//!
//! | File | Task | What it is |
//! |---|---|---|
//! | `mod.rs` | 3.2 | `run()`: paths, single instance, DB, supervisor, runtime, listener |
//! | `rpc.rs` | 3.1 | The protocol, the endpoint, the listener, and the client's way in |
//! | `snapshot.rs` | 3.2 | The event stream's position and the state a `hello` answers with |
//! | `pump.rs` | 3.3 | Win32 message loop + tray, moved off Tauri's event loop |
//! | `spawn.rs` | 3.5 | UI-side helper: start the daemon if no pipe answers |
//!
//! ## No Tauri, and what that costs
//!
//! The daemon builds no `tauri::App`, so everything the UI resolves through an
//! `AppHandle` has to be resolved here instead: the data directory, the
//! recordings folder, ffmpeg, the libobs worker. `paths` does that, mirroring
//! Tauri's own rules — see `Paths::resolve`, and the test that pins the one
//! value both sides have to agree on.
//!
//! `tauri::async_runtime` is the exception, and is not a window: it is a thin
//! wrapper over a global tokio runtime, which the supervisor already spawns
//! its watchers onto. `run` points it at the daemon's own runtime rather than
//! letting it build a second one.
//!
//! ## What is not here yet
//!
//! The tray and its message pump (3.3), autostart (3.5), the updater (3.6),
//! and desktop notifications — which the ownership table gives the daemon, and
//! which need a Tauri-free notifier to land with the tray that gives this
//! process a Win32 presence at all. Until 3.5, nothing starts a daemon
//! automatically: `--daemon` is something you run, and the UI still holds its
//! own supervisor. Running both at once means two processes watching for the
//! same game, which is exactly the state 3.5 exists to end.

#[cfg(windows)]
pub mod pipe_acl;
pub mod notify;
pub mod pump;
pub mod autostart;
pub mod rpc;
pub mod snapshot;
pub mod update;
pub mod spawn;
mod log_bridge;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::contract::events::{Event, LibraryChangeReason, ShutdownReason};
use crate::recorder::Recorder;
use crate::recorder::backend::{self as capture, CaptureBackend, CaptureBackendOption};
use crate::{core, db, fixtures, log, match_summary, retention, state_machine, trim};
use crate::{error, info, warn};

/// The application identifier, and the one string the daemon and the UI must
/// resolve identically.
///
/// Tauri derives `app_data_dir()` from `identifier` in `tauri.conf.json`, and
/// the daemon has no Tauri to ask — so it is repeated here, and
/// `the_identifier_matches_tauri_conf` reads the config back and fails if the
/// two ever differ. Getting this wrong would not crash anything: the daemon
/// would open a *different* database in a *different* folder and record
/// perfectly into a library the UI cannot see.
pub const IDENTIFIER: &str = "com.ninjarecorder.app";

/// Why the daemon could not start.
#[derive(Debug, thiserror::Error)]
pub enum DaemonError {
    /// No `dirs::data_dir()`. On Windows that means no `%APPDATA%`, which
    /// means something is wrong with the profile rather than with us.
    #[error("no application data directory on this system")]
    NoDataDir,
    #[error("cannot create {path}: {source}")]
    DataDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("cannot open the VOD library at {path}: {source}")]
    Db {
        path: PathBuf,
        #[source]
        source: db::DbError,
    },
    #[error("cannot start the async runtime: {0}")]
    Runtime(#[source] std::io::Error),
    #[error("cannot listen on {endpoint}: {source}")]
    Listen {
        endpoint: String,
        #[source]
        source: std::io::Error,
    },
}

/// How long the daemon waits, after saying it is going away, for its sessions
/// to put that on the wire.
///
/// Short because nothing is being waited *for* — the frame is already in each
/// session's channel and a local socket write is microseconds. It is a yield
/// with a floor, not a timeout.
const GOODBYE_GRACE: std::time::Duration = std::time::Duration::from_millis(100);

/// Everything the UI would ask an `AppHandle` for.
///
/// Resolved once, at startup, and then owned — the same shape `Ctx` already
/// uses, and for the same reason: a path re-derived per call is a path that
/// can change answer halfway through a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    /// `app_data_dir()`: the database, the logs, the recordings, the caches.
    pub data: PathBuf,
    pub logs: PathBuf,
    pub recordings: PathBuf,
    pub db: PathBuf,
    pub assets: PathBuf,
    pub fixtures: PathBuf,
    /// The bundled ffmpeg, if this build has one. `None` degrades the
    /// faststart remux and stem extraction rather than stopping a recording,
    /// exactly as it does in the UI.
    pub ffmpeg: Option<PathBuf>,
}

impl Paths {
    /// Mirrors what Tauri would resolve, without building a Tauri app.
    ///
    /// Two rules, both taken from Tauri's own source rather than guessed at:
    ///
    /// - `app_data_dir()` is `dirs::data_dir()` joined with the identifier.
    ///   The `dirs` crate is the one Tauri itself uses and is already in the
    ///   tree through it, so this is the same function producing the same
    ///   answer, not a second implementation of the same rule.
    /// - `BaseDirectory::Resource` is the executable's own directory on
    ///   Windows, which is where the bundle stages `libobs/`.
    pub fn resolve() -> Result<Paths, DaemonError> {
        let data = dirs::data_dir().ok_or(DaemonError::NoDataDir)?.join(IDENTIFIER);
        Ok(Paths {
            logs: data.join("logs"),
            recordings: data.join("recordings"),
            db: data.join("library.sqlite3"),
            assets: data.join("ddragon"),
            fixtures: data.join("fixtures"),
            ffmpeg: ffmpeg(),
            data,
        })
    }
}

/// The directory the bundle stages resources into, or `None` in the odd case
/// where this process cannot name its own executable.
///
/// Windows-only because nothing is staged anywhere else: off Windows there is
/// no libobs worker and no bundled ffmpeg, so there is no resource to find.
#[cfg(target_os = "windows")]
fn resource_dir() -> Option<PathBuf> {
    std::env::current_exe().ok()?.parent().map(std::path::Path::to_path_buf)
}

#[cfg(target_os = "windows")]
fn ffmpeg() -> Option<PathBuf> {
    let path = resource_dir()?.join("libobs").join("ffmpeg.exe");
    path.exists().then_some(path)
}

#[cfg(not(target_os = "windows"))]
fn ffmpeg() -> Option<PathBuf> {
    // Nothing is bundled off Windows; a locally installed ffmpeg is what makes
    // the trim and stem paths exercisable on a dev box.
    crate::which_ffmpeg()
}

/// The capture backends this build and this OS can offer, and how to build
/// each: the I/O half of the `capture_backend` switch (WS1.7). Which one is
/// built is `recorder::backend::choose`'s decision, not this type's.
///
/// Stateless: the worker path is looked up per call, so a repair install that
/// puts the worker back is offered without a restart. A backend that cannot
/// be built is reported, not hidden, and a chosen one that cannot be built
/// becomes a `FailedRecorder` rather than taking the process down, because
/// the library, retention and the LCU watchers all work without it.
struct DaemonBackends;

impl DaemonBackends {
    /// The libobs worker, if it is staged beside the executable.
    #[cfg(target_os = "windows")]
    fn libobs_worker() -> Result<PathBuf, String> {
        resource_dir()
            .map(|dir| dir.join("libobs").join("extprocess_recorder.exe"))
            .filter(|path| path.exists())
            .ok_or_else(|| "the libobs worker is not beside the executable".to_string())
    }

    /// Off Windows there is no libobs and never was: the `libobs` choice
    /// builds `StubRecorder`, which is what this dev loop has always recorded
    /// with, so the setting changes nothing on a Linux or macOS box.
    #[cfg(not(target_os = "windows"))]
    fn libobs_worker() -> Result<PathBuf, String> {
        Ok(PathBuf::new())
    }
}

impl crate::recorder::backend::Backends for DaemonBackends {
    fn options(&self) -> Vec<CaptureBackendOption> {
        vec![
            CaptureBackendOption {
                backend: CaptureBackend::Libobs,
                unavailable: Self::libobs_worker().err(),
            },
            // **WS1.6 replaces this entry** with a real availability check, in
            // the same change that fills `recorder/own/` and flips
            // `CaptureBackend`'s default.
            CaptureBackendOption {
                backend: CaptureBackend::Own,
                unavailable: Some(capture::OWN_NOT_BUILT.to_string()),
            },
        ]
    }

    fn build(&self, backend: CaptureBackend) -> Box<dyn Recorder> {
        match backend {
            #[cfg(target_os = "windows")]
            CaptureBackend::Libobs => match Self::libobs_worker() {
                Ok(worker) => {
                    Box::new(crate::recorder::libobs::LibObsRecorder::new(worker, ffmpeg()))
                }
                Err(why) => Box::new(crate::recorder::FailedRecorder(why)),
            },
            #[cfg(not(target_os = "windows"))]
            CaptureBackend::Libobs => Box::new(crate::recorder::stub::StubRecorder::new()),
            // `choose` never lets this through while the option above says
            // unavailable, so this arm is the refusal said twice rather than a
            // path anything takes. WS1.6 constructs the own backend here.
            CaptureBackend::Own => Box::new(crate::recorder::FailedRecorder(
                capture::OWN_NOT_BUILT.to_string(),
            )),
        }
    }
}

/// Run as the headless recorder daemon. Returns on shutdown, or immediately
/// with the reason it could not start.
///
/// **`Ok(())` also covers "one is already running."** A second `--daemon`
/// launch finds the endpoint owned and leaves quietly, exit code 0: it must
/// never signal the first to quit, because the first might be recording
/// (implementation plan §3.2, startup rule 3). Exiting non-zero would make a
/// perfectly correct login start look like a failure.
pub fn run() -> Result<(), DaemonError> {
    // The one failure with nowhere to go. Everything after this point is
    // recorded in `daemon.log`; a `Paths::resolve` that fails means there is no
    // directory to put a log in, so `main.rs`'s stderr is all there is, and in
    // a release build that is nowhere. It needs `dirs::data_dir()` to return
    // `None`, which on Windows means a profile with no `%APPDATA%`.
    let paths = Paths::resolve()?;

    // The runtime before anything that touches the endpoint: creating a named
    // pipe server registers it with the reactor, so it has to happen inside a
    // runtime context.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(DaemonError::Runtime)?;

    // The supervisor spawns its watchers through `tauri::async_runtime`, which
    // builds a runtime of its own the first time it is used. Pointing it here
    // instead is what keeps the daemon to one runtime rather than two, and is
    // the only line in this file that names `tauri` at all.
    tauri::async_runtime::set(runtime.handle().clone());

    // Setup on the runtime, because binding the endpoint registers it with the
    // reactor. What comes back is everything the rest of this function needs.
    let Some(daemon) = runtime.block_on(start(paths))? else {
        // Another daemon owns the endpoint. Startup rule 3: leave quietly.
        return Ok(());
    };

    // The accept loop runs on the runtime for the daemon's whole life. It ends
    // when `shutdown` fires, which is the only thing that ends it.
    let (shutdown, ends) = tokio::sync::oneshot::channel::<ShutdownReason>();
    let serving = runtime.spawn(accept_until_shutdown(
        daemon.listener,
        Arc::clone(&daemon.ctx),
        daemon.events.clone(),
        ends,
    ));

    // **The main thread belongs to whoever needs it.** On Windows that is the
    // Win32 message pump, because a tray icon's messages arrive on the thread
    // that created it and nothing else may own that thread. Everywhere else
    // there is no tray, so the main thread just waits for Ctrl-C.
    let reason = wait_for_shutdown(&runtime, &daemon.ctx, &daemon.events, shutdown);

    runtime.block_on(async move {
        // Stop taking clients before saying goodbye, so nothing connects
        // between the two and is told nothing.
        let _ = serving.await;
        finish(&daemon.ctx, &daemon.events, reason).await;
    });
    Ok(())
}

/// A daemon that is up: everything the main thread needs a handle on.
struct Started {
    listener: rpc::Listener,
    ctx: Arc<core::Ctx>,
    events: snapshot::Stream,
}

/// Blocks the main thread until something asks the daemon to stop, and returns
/// why.
///
/// Signalling `shutdown` is what ends the accept loop; this owns the only
/// sender, so there is exactly one way out.
fn wait_for_shutdown(
    runtime: &tokio::runtime::Runtime,
    ctx: &Arc<core::Ctx>,
    events: &snapshot::Stream,
    shutdown: tokio::sync::oneshot::Sender<ShutdownReason>,
) -> ShutdownReason {
    let reason = pump_until_quit(runtime, ctx, events);
    let _ = shutdown.send(reason);
    reason
}

/// The tray, on the main thread, until Quit.
///
/// Every menu click arrives here as a `TrayCommand`; the work each one implies
/// is handed elsewhere, because this thread is the message queue and anything
/// slow on it freezes the tray.
///
/// Compiled on every platform, with `pump::run` reporting "no tray here" off
/// Windows and this falling through to the Ctrl-C wait. That is not politeness
/// to other platforms: it is what puts this function in front of the compiler
/// on the box it is written on, which four Windows-only build failures in one
/// day argued for.
fn pump_until_quit(
    runtime: &tokio::runtime::Runtime,
    ctx: &Arc<core::Ctx>,
    events: &snapshot::Stream,
) -> ShutdownReason {
    let (tx, rx) = std::sync::mpsc::channel::<pump::TrayCommand>();

    // The commands are handled on a thread of their own rather than inline in
    // the pump: `pump::run` does not return until `WM_QUIT`, so there is no
    // "after the loop" to drain them in.
    let handler_ctx = Arc::clone(ctx);
    let handler_events = events.clone();
    std::thread::spawn(move || {
        for command in rx {
            match command {
                pump::TrayCommand::ShowUi(view) => {
                    show_ui(&handler_events, view);
                }
                pump::TrayCommand::Quit => {
                    // `is_recording` locks the recorder and returns; no runtime
                    // needed, and no `await` to hold anything across.
                    let recording = crate::core::is_recording(&handler_ctx).unwrap_or(false);
                    if pump::should_confirm_quit(recording)
                        && !pump::confirm_quit_while_recording()
                    {
                        info!("tray", "quit cancelled: a recording is in flight");
                        continue;
                    }
                    pump::stop();
                }
            }
        }
    });

    if let Err(e) = pump::run(tx) {
        // On Windows this is a failure: no tray means no way to reach the app,
        // though the daemon still records, which is the thing worth keeping.
        // Off Windows it is the expected answer and not worth an ERROR in a
        // log someone is reading for real problems.
        if cfg!(windows) {
            error!("tray", "{e}");
        } else {
            info!("tray", "{e}");
        }
        // Without a pump there is nothing to end, so wait the way a
        // trayless platform does.
        match runtime.block_on(tokio::signal::ctrl_c()) {
            Ok(()) => info!("daemon", "interrupted, shutting down"),
            Err(e) => {
                error!("daemon", "cannot listen for Ctrl-C either: {e}");
                runtime.block_on(std::future::pending::<()>());
            }
        }
    }
    ShutdownReason::Quit
}

/// Asks whatever UI is listening to show itself, or starts one.
///
/// Published rather than sent, because the daemon does not track which client
/// is the main window. If nothing is subscribed there is no UI to ask, and the
/// tray's Open has to mean "start one" or it means nothing at all.
fn show_ui(events: &snapshot::Stream, view: Option<String>) {
    if events.has_subscribers() {
        events.publish(Event::ShowUi { view });
        return;
    }

    info!("tray", "no UI is connected; starting one");
    if let Err(e) = spawn::start_ui() {
        error!("tray", "could not start the UI: {e}");
    }
}

async fn start(paths: Paths) -> Result<Option<Started>, DaemonError> {
    // Before the endpoint, because on a dev box the endpoint is a socket
    // *inside* this directory and `bind` fails with a bare `No such file or
    // directory` if it is not there — which on a fresh machine is every first
    // run. Fatal if it fails: the database, the logs and the recordings all
    // live here, so there is nothing useful left to do.
    std::fs::create_dir_all(&paths.data)
        .map_err(|source| DaemonError::DataDir { path: paths.data.clone(), source })?;

    // **The log comes first, before anything that can fail.**
    //
    // It used to come after the bind, so that a second daemon finding the
    // endpoint owned would touch nothing. The cost of that ordering was
    // discovered the hard way: a daemon that died before the bind left no
    // trace anywhere, and on Windows `main.rs`'s stderr goes nowhere in a
    // release build, so the only symptom was a window that flashed and closed.
    // An hour went into finding that out, from a machine that could not run it.
    //
    // Opening the file is not what would have disturbed a running daemon's
    // history anyway. Rotation happens on *write*, past 5 MiB, and the quiet
    // path below writes nothing: it opens the file, finds the endpoint owned,
    // and exits. So the property that ordering protected is kept, and every
    // failure after this line is recorded.
    //
    // **Opened here, announced after the bind.** Saying "logging to ..." at
    // this point would put a line in the file on the quiet path too, which is
    // the property this is meant to keep. Found by checking rather than by
    // reasoning: a second daemon grew the log by 112 bytes.
    let log_file = log::init(&paths.logs, log::Process::Daemon);
    if log_file.is_none() {
        eprintln!("[log] could not open a log file; this session logs to stderr only");
    }

    let endpoint = rpc::endpoint(&paths.data);
    let listener = match rpc::Listener::bind(&endpoint) {
        Ok(Some(listener)) => listener,
        // Another daemon owns it. Nothing is written, which is the whole of
        // what the old ordering was protecting.
        Ok(None) => return Ok(None),
        Err(source) => {
            error!("daemon", "cannot listen on {}: {source}", endpoint.display());
            return Err(DaemonError::Listen { endpoint: endpoint.display().to_string(), source });
        }
    };

    if let Some(path) = log_file {
        info!("daemon", "logging to {}", path.display());
    }
    // What the capture crates log through the `log` facade, libobs's own info
    // and warnings among it, into our files rather than nowhere (#221). After
    // the bind, like the line above: a second daemon that is about to leave
    // quietly has nothing to receive.
    log_bridge::install();
    // Which binary this session is, on the first line it writes. The file
    // name already separates the builds (#202), but a reinstall, an update or
    // a second daemon of the same build only shows up as a new pid or version.
    info!("daemon", "ninja-recorder {} ({} build), pid {}",
        env!("CARGO_PKG_VERSION"),
        rpc::BUILD,
        std::process::id()
    );
    info!("daemon", "listening on {}", endpoint.display());

    // Before the supervisor starts polling — see `fixtures::set_base_dir`.
    fixtures::set_base_dir(paths.fixtures.clone());
    fixtures::init_from_env();
    if fixtures::enabled() {
        info!("fixtures", "capturing API responses to {}", paths.fixtures.display());
    }

    let db = Arc::new(db::Db::open(&paths.db).map_err(|source| {
        // Every reason is fatal and none of them is visible without this:
        // `SchemaTooNew` after a downgrade is the one a person can act on, and
        // the daemon has no window to say so in.
        error!("db", "cannot open the VOD library at {}: {source}", paths.db.display());
        DaemonError::Db { path: paths.db.clone(), source }
    })?);

    // **Read once, here, and then only replaced by `set_capture_backend`.**
    // The setting is chosen in Settings and applied to the next recording by
    // swapping the box inside `recorder`, so nothing re-reads it per game.
    //
    // An unreadable setting is the default, loudly, rather than no backend:
    // the database opened a line ago, so this is a bad read rather than a
    // missing library, and refusing to record over it would be the worse
    // failure.
    let setting = db.get_capture_backend().unwrap_or_else(|e| {
        error!("recorder", "cannot read capture_backend, using the default: {e}");
        CaptureBackend::default()
    });
    let backend = capture::construct(setting, &DaemonBackends);
    // Which backend you get depends on the setting, the OS and whether the
    // chosen backend can be built here, and the difference decides whether
    // recording works at all.
    info!(
        "recorder",
        "backend: {} (capture_backend = {})",
        backend.backend_name(),
        setting.as_pref()
    );
    let recorder: Arc<Mutex<Box<dyn Recorder>>> = Arc::new(Mutex::new(backend));

    let supervisor = state_machine::Supervisor::new(
        Arc::clone(&recorder),
        paths.recordings.clone(),
        Arc::clone(&db),
    );
    let events = snapshot::Stream::new(rpc::Events::new());

    // Every contract event the supervisor produces, onto the wire. This is the
    // whole of what a connected client sees happen.
    {
        let events = events.clone();
        supervisor.set_event_sink(Box::new(move |event| events.publish(event)));
    }

    // Startup housekeeping, in the order `lib.rs` does it and for the same
    // reasons: a folder scan the app missed while it was closed, then a
    // retention pass that last session's crash may have skipped.

    // **First, and only here** (#150). A recording in flight when the last
    // daemon died left a row with no `finished_at`, holding the markers
    // written during the game. That row is hidden from the library and from
    // `reconcile`'s orphan sweep, and its file is skipped by the import pass,
    // so nothing else can ever reach it.
    //
    // It runs before the supervisor exists, which is what makes it safe: an
    // in-progress recording looks exactly like an abandoned one from the
    // database, and finishing the wrong one would put a half-written file in
    // the library. At this point in startup there is nothing recording to
    // confuse it with. The on-demand rescan deliberately does not call this.
    match db::reconcile::recover_unfinished(&db, paths.ffmpeg.as_deref()) {
        Ok(report) if report.recovered > 0 || report.abandoned_removed > 0 => {
            info!(
                "db",
                "startup recovery: finished {} interrupted recording(s), removed {} with no file",
                report.recovered,
                report.abandoned_removed
            );
            events.publish(Event::LibraryChanged { reason: LibraryChangeReason::Reconciled });
        }
        Ok(_) => {}
        Err(e) => error!("db", "startup recovery failed: {e}"),
    }

    match db::reconcile::reconcile(&db, &paths.recordings, paths.ffmpeg.as_deref()) {
        Ok(report) if report.orphans_removed > 0 || report.imported > 0 => {
            info!(
                "db",
                "startup reconcile: removed {} orphan row(s), imported {} untracked file(s)",
                report.orphans_removed,
                report.imported
            );
            events.publish(Event::LibraryChanged { reason: LibraryChangeReason::Reconciled });
        }
        Ok(_) => {}
        Err(e) => error!("db", "startup reconcile failed: {e}"),
    }

    match db.get_retention_policy() {
        Ok(policy) => match retention::enforce_now(&db, &policy) {
            Ok(report) if !report.deleted.is_empty() => {
                info!(
                    "retention",
                    "startup enforcement: removed {} recording(s), freed {} bytes",
                    report.deleted.len(),
                    report.freed_bytes
                );
                events.publish(Event::RetentionRan {
                    deleted: report.deleted.clone(),
                    freed_bytes: report.freed_bytes,
                });
            }
            Ok(_) => {}
            Err(e) => error!("retention", "startup enforcement failed: {e}"),
        },
        Err(e) => error!("retention", "failed to load policy: {e}"),
    }

    wire_finalize_work(&supervisor, &events, &db, paths.ffmpeg.clone());
    supervisor.start();

    // **`new_cyclic`, because one of the seams needs the `Ctx` it lives in.**
    // The update requester has to reach the supervisor and the database to
    // decide and to install, and `Ctx` owns the closure, so an owning handle
    // would be a cycle that neither end ever drops. `new_cyclic` hands out a
    // `Weak` before the value exists, which is the one way to close that loop
    // without leaking it.
    let ctx = Arc::new_cyclic(|weak: &std::sync::Weak<core::Ctx>| {
        let mut ctx = core::Ctx::new(
            recorder,
            Arc::clone(&supervisor),
            db,
            paths.recordings.clone(),
            paths.assets.clone(),
            paths.ffmpeg.clone(),
        );
        {
            let events = events.clone();
            ctx.set_library_changed_notifier(Box::new(move || {
                // `Edited` because this seam is the one a *command* pulls: the
                // finalize, reconcile and retention paths publish their own
                // reason rather than coming through here.
                events.publish(Event::LibraryChanged { reason: LibraryChangeReason::Edited });
            }));
        }
        if crate::updates_enabled() {
            ctx.set_update_requester(update::requester(weak.clone(), events.clone()));
        }
        // How `quit_recorder` stops this process. Ending the message loop is
        // the whole of it: `pump_until_quit` returns, `wait_for_shutdown`
        // signals the accept loop, and `finish` finalizes whatever is being
        // recorded before the process exits. Exactly the path the tray's own
        // Quit takes, which is the point: there is one way for this daemon to
        // stop and both doors open onto it.
        //
        // Off Windows there is no message loop and `pump::stop` is a no-op, so
        // the command reports that it is shutting down and the daemon carries
        // on waiting for Ctrl-C. That is a dev-box shape rather than a shipped
        // one: `pump::run` already refused on this platform and said so.
        ctx.set_quit_requester(Box::new(pump::stop));
        // What `set_capture_backend` may choose between, from the same type
        // the startup backend above was built from, so the two cannot
        // disagree about what this build offers.
        ctx.set_backends(Box::new(DaemonBackends));
        // Start-on-login. `None` here is what the settings row renders as
        // "not available in this build", which is what it said on every build
        // between WS3.4 and #151: the commands moved to this process and the
        // seam stayed in the window.
        match autostart::RegistryAutostart::resolve() {
            Ok(autostart) => {
                // Before the seam is handed over, because this is about the
                // entry on disk rather than about the command that reads it:
                // an entry written before WS3.5 still names `--hidden`, which
                // no longer means anything, and this is what stops that
                // costing a window at every login rather than at one (#71).
                autostart.refresh_if_enabled();
                ctx.set_autostart(Box::new(autostart));
            }
            // Left unset rather than failing the daemon. Not being able to
            // name our own executable is not a reason to refuse to record;
            // the row goes back to saying the control is unavailable, which is
            // then true.
            Err(e) => warn!("autostart", "start-on-login is unavailable: {e}"),
        }
        ctx
    });

    // What a person is told about a game. The daemon's job for the reason the
    // split exists: a notification is for the moment nobody is looking at a
    // window, which is exactly when the UI is not running.
    //
    // **A `Weak`, not an `Arc`.** `Ctx` holds the supervisor and the supervisor
    // would hold this closure, so an owning handle would be a cycle: two
    // objects keeping each other alive for the life of the process and dropped
    // by neither. Upgrading per event costs nothing at the rate these fire, and
    // `None` means the daemon is already tearing down, which is not a moment to
    // raise a toast.
    {
        let ctx = Arc::downgrade(&ctx);
        supervisor.set_event_notifier(Box::new(move |event| {
            if let Some(ctx) = ctx.upgrade() {
                notify::on_supervisor_event(&ctx, event);
            }
        }));
    }

    // The update check, which belongs here for the reason the whole split
    // does: whether an install may run is decided by whether a game is being
    // recorded, and this is the process that knows.
    update::spawn_checks(Arc::clone(&ctx), events.clone());

    Ok(Some(Started { listener, ctx, events }))
}

/// Says goodbye and finalizes, in that order.
///
/// The plan's shutdown order: stop taking clients (the caller has already), say
/// why, then finalize. A recording in flight is worth more than a fast exit,
/// because losing it would leave a fragmented MP4 with no row, recoverable only
/// by the next startup's reconcile and stripped of its markers.
async fn finish(ctx: &Arc<core::Ctx>, events: &snapshot::Stream, reason: ShutdownReason) {
    events.publish(Event::DaemonShuttingDown { reason });
    // Long enough for the session tasks to write that frame. They each own
    // their own connection and are dropped with the runtime the moment this
    // returns, so without a pause the goodbye is a frame that was published and
    // never sent, and a UI that is told nothing shows a dead pipe instead of a
    // reason.
    tokio::time::sleep(GOODBYE_GRACE).await;

    let supervisor = Arc::clone(&ctx.supervisor);
    let finalized = tokio::task::spawn_blocking(move || supervisor.finalize_for_shutdown())
        .await
        .unwrap_or(false);
    if finalized {
        info!("daemon", "finalized an in-flight recording before exiting");
    }
    info!("daemon", "stopped");
}

/// Serves clients until something asks the process to stop.
///
/// **What "something" is belongs to the caller**, which owns the main thread
/// and therefore knows: a tray Quit on Windows, Ctrl-C everywhere else, and
/// WS3.6's "an installer is about to replace this binary". This loop only needs
/// to hear that it happened.
async fn accept_until_shutdown(
    mut listener: rpc::Listener,
    ctx: Arc<core::Ctx>,
    events: snapshot::Stream,
    ends: tokio::sync::oneshot::Receiver<ShutdownReason>,
) {
    let ctx = &ctx;
    let events = &events;
    tokio::pin!(ends);
    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok(stream) => {
                    let ctx = Arc::clone(ctx);
                    let subscription = events.events();
                    let source = events.source(Arc::clone(&ctx));
                    // One task per connection, and nothing shared but the
                    // context: a client that stops reading stalls itself.
                    tokio::spawn(async move {
                        if let Err(e) = rpc::serve(stream, ctx, subscription, source).await {
                            warn!("rpc", "session ended: {e}");
                        }
                    });
                }
                // Not fatal: one refused connection is not a reason to stop
                // recording. A listener that fails every time spins, which the
                // log makes visible.
                Err(e) => warn!("rpc", "could not accept a client: {e}"),
            },
            // A closed channel means the sender went away without sending,
            // which nothing does on purpose; treat it as a stop rather than
            // looping on a dead future.
            _ = &mut ends => return,
        }
    }
}

/// The three callbacks a finalize fires, which all need the async runtime and
/// therefore cannot live in the supervisor.
///
/// Each must return immediately: they are called from inside `stop_recording`,
/// under the recorder lock, where blocking would hold up the 1 Hz status poll
/// and the start of the next game.
fn wire_finalize_work(
    supervisor: &Arc<state_machine::Supervisor>,
    events: &snapshot::Stream,
    db: &Arc<db::Db>,
    ffmpeg: Option<PathBuf>,
) {
    if let Some(ffmpeg) = ffmpeg {
        let db = Arc::clone(db);
        let events = events.clone();
        supervisor.set_trim_requester(Box::new(move |recording_id| {
            let db = Arc::clone(&db);
            let ffmpeg = ffmpeg.clone();
            let events = events.clone();
            tokio::task::spawn_blocking(move || match trim::trim_recording(&db, &ffmpeg, recording_id) {
                Ok(report) => {
                    // `head_removed_s` is a *measured* difference rather than a
                    // decision, so a recording that began at or after the game
                    // reports a few hundredths of a second instead of a clean
                    // zero. Nothing was cut from the front in that case, and
                    // `-0.0s loading screen` reads as a fault in a log that is
                    // scanned for faults, so the two cases are said
                    // differently (trim.rs's header has the reasoning).
                    let head = if report.head_removed_s > 0.05 {
                        format!("{:.1}s loading screen", report.head_removed_s)
                    } else {
                        "no loading screen".to_string()
                    };
                    info!(
                        "trim",
                        "cut {:.1}s off recording {recording_id} ({head}, {:.1}s post-game)",
                        report.removed_s,
                        report.tail_removed_s
                    );
                    // The card's length and size both changed.
                    events.publish(Event::LibraryChanged {
                        reason: LibraryChangeReason::Finalized,
                    });
                }
                // Includes the ordinary "nothing to cut", which is what a
                // reconnect and a client that reported the game late look like.
                Err(e) => crate::debug!("trim", "no trim for recording {recording_id}: {e}"),
            });
        }));
    }

    // Finishes patches an app exit interrupted, once a client is reachable
    // again. The gold curve in particular is written by the deferred patch and
    // by nothing else, so without this a quit inside its one-minute window
    // loses it for good.
    {
        let db = Arc::clone(db);
        let events = events.clone();
        supervisor.set_summary_resumer(Box::new(move |lockfile| {
            let db = Arc::clone(&db);
            let events = events.clone();
            tokio::spawn(async move {
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0);
                if match_summary::resume_pending(&db, &lockfile, now_ms).await > 0 {
                    events.publish(Event::LibraryChanged {
                        reason: LibraryChangeReason::Edited,
                    });
                }
            });
        }));
    }

    {
        let db = Arc::clone(db);
        let events = events.clone();
        supervisor.set_summary_fetcher(Box::new(move |request| {
            let db = Arc::clone(&db);
            let events = events.clone();
            tokio::spawn(async move {
                if match_summary::patch(&db, &request).await {
                    // Nothing else will say so: the row changed minutes after
                    // the library last looked at it.
                    events.publish(Event::MatchSummaryPatched {
                        recording_id: request.recording_id,
                    });
                }
            });
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one value the daemon and the UI must resolve identically, pinned
    /// against the file Tauri reads.
    ///
    /// A mismatch is not a crash but something worse: two processes, two data
    /// directories, two databases, and a daemon recording flawlessly into a
    /// library the UI has never heard of.
    #[test]
    fn the_identifier_matches_tauri_conf() {
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../../tauri.conf.json")).unwrap();
        assert_eq!(
            conf["identifier"].as_str(),
            Some(IDENTIFIER),
            "daemon::IDENTIFIER and tauri.conf.json's identifier decide the same directory"
        );
    }

    /// `tauri.devtools.conf.json` overrides `productName` and must never
    /// override `identifier`: the endpoint is scoped by build identity
    /// separately, and a devtools build that moved its data directory would
    /// stop sharing the library the portal exists to inspect.
    #[test]
    fn the_devtools_config_does_not_move_the_data_directory() {
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../../tauri.devtools.conf.json")).unwrap();
        assert!(
            conf.get("identifier").is_none(),
            "a devtools identifier would split the data directory in two"
        );
    }

    /// Every path the daemon uses hangs off the data directory, which is what
    /// makes "the UI and the daemon agree" a single question rather than six.
    #[test]
    fn every_path_sits_under_the_data_directory() {
        let Ok(paths) = Paths::resolve() else {
            // No `dirs::data_dir()` on this machine; nothing to check.
            return;
        };
        assert!(paths.data.ends_with(IDENTIFIER));
        for path in [&paths.logs, &paths.recordings, &paths.db, &paths.assets, &paths.fixtures] {
            assert!(
                path.starts_with(&paths.data),
                "{} escaped the data directory",
                path.display()
            );
        }
    }
}
