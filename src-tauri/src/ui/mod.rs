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
