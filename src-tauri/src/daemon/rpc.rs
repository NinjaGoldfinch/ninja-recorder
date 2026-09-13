//! JSON-RPC server over a named pipe.
//!
//! **Empty — WS3 task 3.1.** Listener, newline-delimited framing, per-session
//! state and event subscriptions. The command half is `core::dispatch`, which
//! already exists and names no `tauri` type; the event half is WS2's generated
//! `Event` enum (implementation plan §4.2).
//!
//! WS3: the pipe name must be scoped by build identity — see the note in
//! `lib.rs` where `app_data_dir()` is first read.
