//! The capture worker: `ninja-recorder.exe --capture-worker`, the process
//! the own backend's session thread runs in (#241).
//!
//! **Why a process at all.** Capture already runs on its own threads, so it
//! never blocks the daemon. What a process adds is crash isolation: a driver
//! fault inside an encoder MFT (the class of crash #218 fixed in the spike)
//! kills the worker, not the daemon in the middle of a game. The recording it
//! was writing is fragmented, so it is playable up to its last fragment, and
//! the daemon keeps it (`OwnRecorder::stop`), as soon as the worker's pipe
//! closes rather than at the end of the game (#299, `client`).
//!
//! **Why not a bin target.** Tauri's bundler installs every bin target this
//! package builds (CLAUDE.md, "The emitter is not built by default"), so a
//! second binary would be a second executable in every install, with its own
//! installer story. A flag on the one binary needs neither, and `main.rs`
//! dispatches it before the UI or the daemon is built: the worker takes no
//! single-instance lock, builds no tray, opens no database and binds no pipe.
//!
//! **When it runs.** Only while League does: spawned by `prepare` (the client
//! opened) or `start`, ended by `release` (the client closed) once no
//! recording is in flight. [`lifetime`] is that rule, pure.
//!
//! - [`protocol`] — the messages, one JSON value per line, shared by both
//!   sides.
//! - [`serve`] — the worker's loop over stdin and stdout.
//! - [`lifetime`] — when the daemon spawns and ends it.
//! - [`client`] — the daemon's side: the child, its job object and its pipes.

pub mod client;
pub mod lifetime;
pub mod protocol;
pub mod serve;

use std::io::Write;
use std::path::PathBuf;

use crate::log;
use crate::{error, info};

/// Where the worker writes `worker.log`: the daemon sets it to its own log
/// directory when it spawns the worker, so the two files are always side by
/// side. Unset, the worker resolves the same directory the daemon would.
pub const LOG_DIR_ENV: &str = "NINJA_CAPTURE_WORKER_LOG_DIR";

/// Runs the worker until the daemon releases it or goes away, and returns
/// the process's exit code: 0 for every orderly end, EOF included.
pub fn run() -> i32 {
    // First, before anything can print: stdout is the channel.
    let channel = take_stdout();

    let logs = std::env::var_os(LOG_DIR_ENV)
        .map(PathBuf::from)
        .or_else(|| crate::daemon::Paths::resolve().ok().map(|paths| paths.logs));
    let log_file = logs.and_then(|dir| log::init(&dir, log::Process::Worker));

    // A panic is written to `worker.log`, and the default hook's report goes
    // to stderr, which the daemon drains into `daemon.log`.
    let report = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic| {
        error!("worker", "capture worker panicked: {panic}");
        report(panic);
    }));

    info!(
        "worker",
        "ninja-recorder {} capture worker, pid {}, logging to {}",
        env!("CARGO_PKG_VERSION"),
        std::process::id(),
        log_file.as_deref().map_or("nowhere".into(), |p| p.display().to_string())
    );

    #[cfg(target_os = "windows")]
    let mut host = super::win::host::SessionHost::default();
    #[cfg(not(target_os = "windows"))]
    let mut host = serve::Refusing;

    let exit = serve::serve(std::io::stdin().lock(), channel, &mut host);
    info!("worker", "capture worker exiting ({exit:?})");
    exit.code()
}

/// The worker's end of the channel, taken from the standard output handle,
/// which is then pointed at stderr so that nothing else can write into the
/// channel: not a stray `println!`, not a library that logs to stdout (#221).
/// Whatever does write there lands in stderr, and so in `daemon.log`, where it
/// can be read.
///
/// Rust's `std::io::stdout` looks the handle up with `GetStdHandle` on every
/// write, so replacing it here reaches every later print.
#[cfg(target_os = "windows")]
fn take_stdout() -> Box<dyn Write> {
    use std::os::windows::io::FromRawHandle;
    use windows::Win32::System::Console::{
        GetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE, SetStdHandle,
    };

    // SAFETY: plain calls, reading this process's standard handles.
    let out = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    // SAFETY: as above.
    let err = unsafe { GetStdHandle(STD_ERROR_HANDLE) };
    match out {
        Ok(out) if !out.is_invalid() && !out.0.is_null() => {
            // SAFETY: plain call. Once it returns, nothing in this process
            // names `out` as its standard output any more.
            let _ = unsafe { SetStdHandle(STD_OUTPUT_HANDLE, err.unwrap_or_default()) };
            // SAFETY: `out` is a live handle this process owns, and after
            // the call above the returned `File` is its only user.
            Box::new(unsafe { std::fs::File::from_raw_handle(out.0) })
        }
        // No stdout at all: the worker was started by hand, not by a daemon.
        // `serve` will find stdin closed, or talk to the console.
        _ => Box::new(std::io::stdout()),
    }
}

/// Off Windows the worker only exists for the process-level test, and
/// nothing in it prints; stdout is used as it is.
#[cfg(not(target_os = "windows"))]
fn take_stdout() -> Box<dyn Write> {
    Box::new(std::io::stdout())
}
