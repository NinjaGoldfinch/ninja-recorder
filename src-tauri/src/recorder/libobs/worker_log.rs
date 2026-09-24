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
//! libobs's default handler (`def_log_handler` in `util/base.c`) splits them
//! by level: **errors go to stderr, and info and warnings to stdout.** The
//! fork's `ipc-link` spawns the worker with stdin and stdout piped, since
//! they carry the JSON IPC protocol, and **stderr inherited**. So the two
//! halves arrive in different places, and each needs its own way into a file.
//!
//! **Errors** already arrive at our stderr, which in a release build (`main.rs`
//! sets `windows_subsystem = "windows"`) has no console behind it. So: point
//! our stderr at a file before the worker spawns, and the child inherits it.
//! No change to the fork, no IPC change, and, writing to a file rather than a
//! pipe, no way to block the worker by failing to drain it. ffmpeg's stderr,
//! from the muxer the worker runs, lands here the same way.
//!
//! **Info and warnings** come up the IPC pipe, interleaved with the replies.
//! `ipc-link` reads each line while it waits for a reply, and hands any line
//! that is not JSON to the `log` crate as `[rec]: ...`, in this process.
//! `daemon::log_bridge` receives those and writes them here through `append`,
//! so all of libobs's output reads as one file (#221). Before that bridge
//! existed nothing received them: this module's first version assumed the
//! default handler wrote everything to stderr, and only the errors ever
//! reached the file.
//!
//! Those lines move only when the daemon talks to the worker, so they reach
//! the file a little after the errors do, and mid-game they wait for
//! `LibObsRecorder::collect_output`, which the supervisor calls every few
//! polls so the pipe never fills and blocks the worker.
//!
//! The costs, stated plainly: the lines land in their own file rather than
//! interleaved with ours, and they arrive without a level to filter on,
//! because they are libobs's formatting and not ours. Both are what a
//! handler inside the worker would fix, and that is a change to the fork
//! (#69, option A) worth making once a real capture shows it is needed.

use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;

/// The file stderr was pointed at, and where it is.
struct Redirect {
    path: PathBuf,
    /// Held for the rest of the process's life, which is what keeps the
    /// handle stderr now names valid: a static is never dropped. Also what
    /// `append` writes through.
    file: File,
}

/// Set once, so the redirect happens exactly once however many times the
/// backend goes cold and warm again.
static REDIRECTED: OnceLock<Option<Redirect>> = OnceLock::new();

/// Points this process's standard error at `logs/libobs.log` (or
/// `libobs-devtools.log` in a devtools build, #202; `log::libobs_file_names`
/// has the reason), so the worker spawned after it inherits the handle.
///
/// Returns where it went, or `None` when nothing was done — which is the
/// normal answer in a debug build, where there *is* a console and taking
/// stderr away from it would be a downgrade.
///
/// Must be called before the worker is spawned. Calling it afterwards
/// changes nothing: a child inherits the handles its parent held at
/// `CreateProcess` time.
pub(super) fn redirect_once() -> Option<&'static PathBuf> {
    REDIRECTED.get_or_init(install).as_ref().map(|r| &r.path)
}

/// Writes one of the worker's stdout lines into the same file as its stderr,
/// for `daemon::log_bridge` (#221). `false` when there is no such file, which
/// is a debug build with a console, or any moment before the worker first
/// spawned; the caller keeps the line somewhere else.
///
/// Through the same file object the worker's inherited handle refers to, so
/// the two share one file position and their lines interleave in the order
/// they were written rather than overwriting each other. `\r\n` because that
/// is what the worker's C runtime writes for libobs's `\n` in text mode, and
/// one file with two line endings reads badly in Notepad.
pub(crate) fn append(line: &str) -> bool {
    let Some(Some(redirect)) = REDIRECTED.get() else {
        return false;
    };
    // One `write_all` of the whole line, so it is not split around the
    // worker's own writes. A failed write is dropped like every other log
    // failure (`crate::log`'s header): recording matters more.
    let _ = (&redirect.file).write_all(format!("{line}\r\n").as_bytes());
    true
}

fn install() -> Option<Redirect> {
    // `debug_assertions` is exactly the condition `main.rs` gates
    // `windows_subsystem = "windows"` on, so this is precisely "there is
    // no console to lose". The override exists so the path can be
    // exercised from `tauri:dev` rather than only in a shipped build.
    if cfg!(debug_assertions) && std::env::var_os("NINJA_RECORDER_LIBOBS_LOG").is_none() {
        return None;
    }

    let dir = crate::log::dir()?;
    let [active, previous] = crate::log::libobs_file_names();
    let path = dir.join(active);

    // One previous session kept, then overwritten. Appending forever would
    // grow unbounded — libobs is chatty at startup — and truncating
    // outright would lose the session that crashed, which is the one
    // anybody is looking for.
    let previous = dir.join(previous);
    let _ = std::fs::remove_file(&previous);
    let _ = std::fs::rename(&path, &previous);

    let file = File::create(&path).ok()?;
    set_stderr(&file)?;
    // Kept in the static rather than closed: the handle has to stay valid
    // for as long as anything writes to stderr, which is the rest of the
    // process's life. One file handle, once.
    Some(Redirect { path, file })
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
    // stores it in a static immediately afterwards, so the handle stays
    // valid for as long as the process might write to stderr. `SetStdHandle` only
    // records the handle; it does not take ownership or close the old one.
    unsafe { SetStdHandle(STD_ERROR_HANDLE, HANDLE(file.as_raw_handle())).ok() }
}
