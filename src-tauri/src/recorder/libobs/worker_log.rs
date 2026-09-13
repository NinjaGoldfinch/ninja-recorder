//! Keeping the libobs worker's own log.
//!
//! ## Why this is not `base_set_log_handler`
//!
//! The obvious fix — install a libobs log handler — does not work here,
//! and the reason is worth writing down because the symbol being available
//! makes it look like it should. `libobs-recorder` runs libobs **out of
//! process**: `LibObs::new_with_paths` spawns `extprocess_recorder.exe`,
//! and `obs_startup` is called *there*. A handler installed from this
//! process would attach to a libobs instance we never initialize, compile
//! and run cleanly, and capture nothing.
//!
//! ## What actually happens to those messages
//!
//! libobs's default handler writes to stderr, and the fork's `ipc-link`
//! spawns the worker with stdin and stdout piped — they carry the JSON IPC
//! protocol — and **stderr inherited**. So the messages already exist and
//! already arrive somewhere: our stderr. In a release build `main.rs` sets
//! `windows_subsystem = "windows"`, so there is no console behind it and
//! they are written to an invalid handle and lost.
//!
//! So: point our stderr at a file before the worker spawns, and the child
//! inherits it. No change to the fork, no IPC change, and — writing to a
//! file rather than a pipe — no way to block the worker by failing to
//! drain it, which is the trap the pipe version has.
//!
//! The costs, stated plainly: the lines land in their own file rather than
//! interleaved with ours, and they arrive without a level to filter on,
//! because they are libobs's formatting and not ours. Both are what a
//! handler inside the worker would fix, and that is a change to the fork
//! (#69, option A) worth making once a real capture shows it is needed.

use std::path::PathBuf;
use std::sync::OnceLock;

/// Set once, so the redirect happens exactly once however many times the
/// backend goes cold and warm again.
static REDIRECTED: OnceLock<Option<PathBuf>> = OnceLock::new();

/// Points this process's standard error at `logs/libobs.log`, so the
/// worker spawned after it inherits the handle.
///
/// Returns where it went, or `None` when nothing was done — which is the
/// normal answer in a debug build, where there *is* a console and taking
/// stderr away from it would be a downgrade.
///
/// Must be called before the worker is spawned. Calling it afterwards
/// changes nothing: a child inherits the handles its parent held at
/// `CreateProcess` time.
pub(super) fn redirect_once() -> Option<&'static PathBuf> {
    REDIRECTED.get_or_init(install).as_ref()
}

fn install() -> Option<PathBuf> {
    // `debug_assertions` is exactly the condition `main.rs` gates
    // `windows_subsystem = "windows"` on, so this is precisely "there is
    // no console to lose". The override exists so the path can be
    // exercised from `tauri:dev` rather than only in a shipped build.
    if cfg!(debug_assertions) && std::env::var_os("NINJA_RECORDER_LIBOBS_LOG").is_none() {
        return None;
    }

    let dir = crate::log::dir()?;
    let path = dir.join("libobs.log");

    // One previous session kept, then overwritten. Appending forever would
    // grow unbounded — libobs is chatty at startup — and truncating
    // outright would lose the session that crashed, which is the one
    // anybody is looking for.
    let previous = dir.join("libobs.1.log");
    let _ = std::fs::remove_file(&previous);
    let _ = std::fs::rename(&path, &previous);

    let file = std::fs::File::create(&path).ok()?;
    set_stderr(&file)?;
    // Deliberately leaked: the handle has to outlive this function and
    // stay valid for as long as anything writes to stderr, which is the
    // rest of the process's life. One file handle, once.
    std::mem::forget(file);
    Some(path)
}

// No `cfg(windows)` here: this whole module hangs off `recorder::libobs`,
// which `recorder/mod.rs` already compiles only on Windows. A second gate
// would just mean a missing function rather than a clear error if that
// ever changed.
fn set_stderr(file: &std::fs::File) -> Option<()> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Console::{SetStdHandle, STD_ERROR_HANDLE};

    // SAFETY: `file` is open for the duration of this call, and the caller
    // leaks it immediately afterwards so the handle stays valid for as
    // long as the process might write to stderr. `SetStdHandle` only
    // records the handle; it does not take ownership or close the old one.
    unsafe { SetStdHandle(STD_ERROR_HANDLE, HANDLE(file.as_raw_handle())).ok() }
}
