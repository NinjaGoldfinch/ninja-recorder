//! The headless recorder daemon.
//!
//! **Not built yet — WS3 (tasks 3.1–3.7).** This module exists now so the
//! seam does: `main.rs` dispatches `Launch::Daemon` here, and `run` refuses.
//! The refusal is a value returned from the daemon rather than a string in
//! `launch.rs`, so filling WS3 in means replacing a function body, not
//! rerouting a process.
//!
//! What lands here, per the implementation plan §3.1 and §4.3:
//!
//! | File | WS3 task | What it becomes |
//! |---|---|---|
//! | `mod.rs` | 3.2 | `run()`: single-instance mutex, DB open, pipe listen, Win32 pump, tokio runtime |
//! | `pump.rs` | 3.3 | Win32 message loop + tray, moved off Tauri's event loop (from `tray.rs`) |
//! | `rpc.rs` | 3.1 | Named-pipe listener, framing, per-session state, event subscriptions |
//! | `snapshot.rs` | 3.4 | Full-state snapshot served on `hello` and on resync after a dropped pipe |
//! | `spawn.rs` | 3.5 | UI-side helper: start the daemon if no pipe answers |
//!
//! The daemon owns everything that must outlive the UI: the supervisor and
//! state machine, the `Recorder` and its capture backend, every SQLite write,
//! the tray, autostart and the updater, and `daemon.log`. It links no Tauri
//! window and no WebView2. See §3.1's ownership table.

pub mod pump;
pub mod rpc;
pub mod snapshot;
pub mod spawn;

/// Why the daemon could not start.
///
/// One variant today. It is an enum rather than a `String` because WS3 adds
/// the real ones — the mutex was already held, the pipe could not be bound,
/// the database would not open — and callers should be matching on them
/// before that, not parsing prose.
#[derive(Debug, thiserror::Error)]
pub enum DaemonError {
    #[error("--daemon is not implemented yet — WS3 builds the daemon process")]
    NotImplemented,
}

/// Run as the headless recorder daemon. Returns only on shutdown, or
/// immediately with the reason it could not start.
///
/// `main.rs` turns an `Err` into a message on stderr and exit code 2, which
/// is what `docs/windows-verification.md` §5.0 checks for `--daemon` — the
/// text goes nowhere in a release build (`windows_subsystem = "windows"`),
/// so the exit code is the observable part and it has not changed from v1.
pub fn run() -> Result<(), DaemonError> {
    Err(DaemonError::NotImplemented)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The seam, pinned: `--daemon` is refused *by the daemon*, not by a
    /// string in `launch.rs`. WS3 deletes this test by making `run` block.
    #[test]
    fn run_refuses_until_ws3_builds_it() {
        let err = run().expect_err("the daemon is not implemented yet");
        assert!(matches!(err, DaemonError::NotImplemented));
        assert!(
            err.to_string().contains("WS3"),
            "the refusal should name the workstream that fixes it, got: {err}"
        );
    }
}
