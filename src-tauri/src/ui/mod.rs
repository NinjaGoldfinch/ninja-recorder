//! The Tauri UI process.
//!
//! **Empty — WS3 (tasks 3.5–3.6).** What is left of `lib.rs` once the daemon
//! owns the supervisor, the recorder, the writes, the tray, autostart and the
//! updater: window setup, the invoke transport that bridges the webview to the
//! pipe, a `query_only` SQLite reader, `open_recordings_folder`, and `ui.log`.
//!
//! The UI is disposable by design. Killing it must not stop a recording and
//! must not lose a marker; everything it holds is either presentation state or
//! a read-only view of what the daemon wrote (implementation plan §3.1, §3.2).
//!
//! `client` is the first piece: the UI's side of the pipe, with reply routing,
//! reconnect and version-skew refusal. There is a daemon to talk to since
//! WS3.2, and `daemon::rpc::connect` opens the address it listens on. What is
//! missing is the rest of 3.4 — the `rpc_call`/`rpc_subscribe` commands that
//! expose this to the webview — and 3.5, which is what makes the UI start a
//! daemon rather than assume one.

pub mod client;
pub mod link;
