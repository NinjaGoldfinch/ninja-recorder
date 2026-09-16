//! The UI's connection to the daemon, and the two commands that expose it to
//! the webview. WS3 task 3.4.
//!
//! `client` is the protocol half: reply routing, reconnect, skew refusal.
//! This is what hangs it off a Tauri app, so the frontend can reach the daemon
//! without knowing a pipe exists.
//!
//! ## Two commands, not thirty
//!
//! `rpc_call` forwards a name and a JSON blob, exactly as `rpc` did when the
//! commands ran in this process. The frontend's transport changes which
//! function it calls and nothing else: the wire shape, the argument spelling
//! and the error strings are the ones `core::dispatch` already produced, so
//! every view keeps working across the move.
//!
//! `rpc_subscribe` is how the webview learns what happened. It does not return
//! a stream: it asks for the current snapshot, and everything after that
//! arrives as the `event` the contract already defines. A window that has just
//! opened therefore does exactly what a reconnecting client does, which is the
//! shape WS2.4 chose the snapshot for.
//!
//! ## The daemon is started, not required
//!
//! `daemon::spawn::connect_or_start` is the connect factory, so a UI launched
//! on a machine with no daemon starts one rather than showing an error. That
//! is the recovery path rather than the normal one: after WS3.5 the Run key
//! starts a daemon at login and this only fires when someone has quit it, it
//! crashed, or this is a first launch.
//!
//! ## No tests live in this file, and none should
//!
//! It names `AppHandle`, `Emitter` and `Manager`, and is reachable only from
//! `lib.rs`'s `run()`, which is dead code in a `cargo test` build and gets
//! stripped. That is what keeps Tauri's Wry window machinery, and with it the
//! whole Win32 GUI import stack, out of the test binary: a `cargo test` binary
//! carries no application manifest, so Windows resolves `comctl32.dll` to the
//! v5 side-by-side assembly and the binary dies at load with
//! `STATUS_ENTRYPOINT_NOT_FOUND` before running anything. `tray.rs` carries the
//! same warning and `state_machine::supervisor` documents the incident.
//!
//! What is worth testing is either side of this file and already is: the
//! protocol and the reconnect are `client`'s tests, against a real server; the
//! routing is `src/lib/transport/pipe.test.ts`. What is left here is the part
//! that needs a window, which is what `docs/windows-verification.md` is for.
//!
//! ## What the UI no longer has
//!
//! No supervisor, no capture backend, no watchers. `lib.rs` used to build all
//! three; killing the window used to kill them with it, which is the whole
//! thing WS3 exists to end. What is left in this process is the window, the
//! tray, the shell commands, and this.

use std::sync::Arc;

use serde_json::Value;
use tauri::{Emitter, Manager};

use crate::daemon::spawn;
use crate::contract::snapshot::Snapshot;
use crate::ui::client::{self, Client, FromDaemon, Health};
use crate::{info, warn};

/// How many events can be queued for the webview before the reader is
/// considered wedged.
///
/// The daemon's own broadcast is already bounded, and this is the second half
/// of the same argument: a window that has stopped reading must not be able to
/// grow this process's memory while a game is being recorded. A UI that falls
/// behind gets a `Lagged` frame and re-`hello`s, which is the same recovery the
/// daemon's subscribers use.
const EVENT_QUEUE: usize = 256;

/// Managed state: the handle the commands below call through.
pub struct DaemonLink {
    client: Client,
    /// The last snapshot the daemon sent, kept for windows that open later.
    ///
    /// The daemon answers `hello` with one, so the connection has a snapshot
    /// from the moment it is made. A second window opening an hour later missed
    /// that handshake, and re-`hello`ing a live connection to serve it would
    /// drop the first window's subscription. Remembering the last one costs a
    /// few hundred bytes and makes a new window's first paint identical to a
    /// reconnecting client's.
    latest: Arc<tokio::sync::Mutex<Option<Snapshot>>>,
}

impl DaemonLink {
    /// Connects, and keeps connecting, for the life of the process.
    ///
    /// Returns immediately. The first `rpc_call` may well land while the
    /// connection is still being made, which is why `Client::call` answers
    /// `Disconnected` rather than blocking: a window opening during a daemon
    /// restart should render its empty state and recover on the next event.
    pub fn connect(app: &tauri::AppHandle, endpoint: std::path::PathBuf) -> DaemonLink {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<FromDaemon>(EVENT_QUEUE);
        let latest: Arc<tokio::sync::Mutex<Option<Snapshot>>> = Arc::default();

        // Every topic. The main window renders recording state, the LCU strip,
        // the library and the update badge, and the daemon's own lifecycle is
        // what tells it the connection is going away on purpose rather than
        // dying. Narrowing this is a per-window decision nothing has asked for.
        let topics = vec![
            crate::contract::events::Topic::Recording,
            crate::contract::events::Topic::Lcu,
            crate::contract::events::Topic::Library,
            crate::contract::events::Topic::Update,
            crate::contract::events::Topic::Daemon,
        ];

        // **Entered, because `setup` is not inside a runtime.**
        //
        // `client::spawn` calls `tokio::spawn`, which panics with "there is no
        // reactor running" unless a runtime is entered on the calling thread.
        // Tauri's `setup` hook runs on the main thread before any of that, so
        // the UI aborted on every single launch: the window never appeared and
        // the process died a few milliseconds after writing its first log line.
        //
        // `client` stays runtime-agnostic on purpose — it is a protocol client,
        // and which runtime it runs on is the caller's business, exactly as
        // `daemon::rpc::serve` is called from inside the daemon's. This is the
        // caller doing its half.
        let runtime = tauri::async_runtime::handle();
        let _entered = runtime.inner().enter();
        let client = client::spawn(
            move || {
                let endpoint = endpoint.clone();
                async move { spawn::connect_or_start(&endpoint).await }
            },
            topics,
            tx,
        );

        // One task draining the loop's output into Tauri events. The webview is
        // the only consumer, and it gets the same `event` channel the contract
        // already defines, so a view that was listening before the split keeps
        // working after it.
        let app = app.clone();
        let store = Arc::clone(&latest);
        tauri::async_runtime::spawn(async move {
            while let Some(message) = rx.recv().await {
                match message {
                    FromDaemon::Snapshot(snapshot) => {
                        // A fresh handshake replaces the frontend's world, so
                        // it goes out on its own channel rather than as one
                        // more event: a client that treated it as an event
                        // would apply it in order rather than instead.
                        if let Err(e) = app.emit(SNAPSHOT_EVENT, &*snapshot) {
                            warn!("ui", "failed to emit a snapshot: {e}");
                        }
                        *store.lock().await = Some(*snapshot);
                    }
                    FromDaemon::Event(event) => {
                        // The tray is in the daemon, so its Open and Settings
                        // arrive here rather than as a menu callback. Handled
                        // before the relays because it is a request rather than
                        // news: nothing downstream needs to see it.
                        if let crate::contract::events::Event::ShowUi { view } = &event {
                            crate::tray::show_window(&app, view.as_deref());
                            continue;
                        }
                        // The v1 channels first, because the views that listen
                        // on them have not been rewritten yet.
                        relay_legacy(&app, &event);
                        if let Err(e) = app.emit(crate::CONTRACT_EVENT, &event) {
                            warn!("ui", "failed to emit a contract event: {e}");
                        }
                    }
                    FromDaemon::Health(health) => {
                        info!("ui", "daemon connection: {health:?}");
                        if let Err(e) = app.emit(HEALTH_EVENT, health_payload(&health)) {
                            warn!("ui", "failed to emit connection health: {e}");
                        }
                    }
                }
            }
        });

        DaemonLink { client, latest }
    }
}

/// Re-emits the two v1 events the existing views still listen on.
///
/// `library.ts` rebuilds its grid on `library-changed` and the About block
/// re-reads on `update-status-changed`. Both used to be emitted by the code
/// that did the thing; that code is in the daemon now, and what arrives here
/// instead is a contract event. Without this bridge the library would simply
/// stop refreshing after a game, which is the kind of regression that reads as
/// "the app feels stale" rather than as a broken feature.
///
/// WS4 deletes this along with those listeners: a view driven by the store that
/// the snapshot and the event stream feed has no use for a second channel
/// saying the same thing. Until then, "frontend behaviour unchanged" is the
/// bar, and this is what holds it.
fn relay_legacy(app: &tauri::AppHandle, event: &crate::contract::events::Event) {
    use crate::contract::events::Event;

    let moved_the_library = matches!(
        event,
        Event::LibraryChanged { .. }
            | Event::RetentionRan { .. }
            | Event::MatchSummaryPatched { .. }
            // The finalize writes the row, so the library moved even though the
            // event is about the recording.
            | Event::RecordingStopped { .. }
    );
    if moved_the_library
        && let Err(e) = app.emit(crate::LIBRARY_CHANGED_EVENT, ())
    {
        warn!("ui", "failed to relay library-changed: {e}");
    }

    if let Event::UpdateStatus { .. } = event
        && let Err(e) = app.emit(crate::UPDATE_STATUS_EVENT, ())
    {
        warn!("ui", "failed to relay update-status-changed: {e}");
    }
}

/// The snapshot channel. Separate from `CONTRACT_EVENT` because a snapshot
/// *replaces* state where an event *updates* it, and a frontend that could not
/// tell them apart would fold a fresh world into a stale one.
pub(crate) const SNAPSHOT_EVENT: &str = "snapshot";

/// The connection's health, for the strip that says "reconnecting" or "restart
/// required". Its own channel for the same reason: it is not something that
/// happened in the recorder, it is something that happened to the wire.
pub(crate) const HEALTH_EVENT: &str = "daemon-health";

/// `Health` as the frontend reads it.
///
/// Rendered here rather than derived on the enum because `Health` is the
/// client's internal vocabulary and this is a wire shape: giving it serde
/// derives would make a refactor of one a silent change to the other.
fn health_payload(health: &Health) -> Value {
    match health {
        Health::Connected => serde_json::json!({ "state": "connected" }),
        Health::Reconnecting => serde_json::json!({ "state": "reconnecting" }),
        Health::Skewed { ours, theirs } => serde_json::json!({
            "state": "skewed",
            "ours": ours,
            "theirs": theirs,
        }),
    }
}

impl DaemonLink {
    /// Runs one command on the daemon, with the error already rendered.
    ///
    /// The one path a command takes out of this process. `lib.rs`'s `rpc`
    /// forwards through it, and so does `rpc_call`, which is the same thing
    /// under the name the plan gives it (§5, task 3.4).
    pub async fn call(&self, command: &str, args: Value) -> Result<Value, String> {
        self.client.call(command, args).await.map_err(|e| e.to_string())
    }
}

/// Runs one command on the daemon.
///
/// Deliberately the same shape as the `rpc` it succeeds: a command name and a
/// JSON blob in, the command's success value or an error string out. What
/// changed is which process runs it.
#[tauri::command]
pub async fn rpc_call(
    state: tauri::State<'_, DaemonLink>,
    command: String,
    args: Value,
) -> Result<Value, String> {
    state.call(&command, args).await
}

/// The current snapshot, for a window that has just opened.
///
/// The *subscription* was made when the connection was: `client::spawn`
/// subscribes on every handshake, because a client that had to ask for its
/// subscription would miss whatever happened between connecting and asking. So
/// what a window actually needs on open is the baseline, and this is it.
///
/// `None` means no handshake has completed yet, which is a real state on a
/// cold start: the daemon is being launched and the window is already painting.
/// The frontend renders its empty state and the `snapshot` event replaces it a
/// moment later, which is the same path a reconnect takes.
#[tauri::command]
pub async fn rpc_subscribe(state: tauri::State<'_, DaemonLink>) -> Result<Option<Snapshot>, String> {
    Ok(state.latest.lock().await.clone())
}

/// Whether the UI is talking to a daemon right now, for the connection strip.
#[tauri::command]
pub async fn rpc_health(state: tauri::State<'_, DaemonLink>) -> Result<Value, String> {
    Ok(health_payload(&state.client.health().await))
}

/// Attaches the link to the app.
///
/// Called once from `setup`, after the app is built, because `connect` needs an
/// `AppHandle` to emit through.
pub fn attach(app: &tauri::AppHandle, endpoint: std::path::PathBuf) {
    app.manage(DaemonLink::connect(app, endpoint));
}
