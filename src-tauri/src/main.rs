// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use ninja_recorder_lib::daemon;
use ninja_recorder_lib::launch::Launch;

/// One binary, two modes. Which one is argv's decision, and it is made here —
/// before either side is built — because the daemon must never construct a
/// Tauri app and the UI must never construct a supervisor.
///
/// The dispatch is in `main.rs` rather than inside `run()` so that the two
/// entry points stay genuinely separate: `daemon::run` links no window, and a
/// future build that wants to drop WebView2 from the daemon has one branch to
/// cut rather than a runtime flag threaded through setup.
///
/// See the implementation plan §3.2 and `launch.rs` for why the flag strings
/// are an on-disk contract rather than an implementation detail.
fn main() {
    match Launch::from_env() {
        Launch::Daemon => {
            if let Err(why) = daemon::run() {
                // Deliberately not through the log facade: this is the
                // daemon refusing to start, so there is no `daemon.log` to
                // write to yet.
                //
                // Exit code 2, unchanged from v1. `windows_subsystem =
                // "windows"` means a release build has no console and this
                // text goes nowhere, which is why
                // `docs/windows-verification.md` §5.0 checks the code and not
                // the message.
                eprintln!("[launch] {why}");
                std::process::exit(2);
            }
        }
        // `UiHidden` creates no window but is otherwise a normal UI start;
        // `run` reads the mode again to decide that. See the plan's §3.5 for
        // why the flag survives one more release rather than being retired
        // the moment `--daemon` works.
        Launch::Ui | Launch::UiHidden => ninja_recorder_lib::run(),
    }
}
