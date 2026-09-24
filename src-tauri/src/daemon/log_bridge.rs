//! A receiver for the `log` crate, so what our dependencies log is kept (#221).
//!
//! This app does not log through `log`: `crate::log` is its own file, and
//! DEVELOPMENT.md §13 says why. But the capture backend's crates do, and
//! nothing had installed a logger to receive it, so every record they made was
//! dropped at the facade.
//!
//! The one that matters is libobs itself. libobs's default log handler writes
//! errors to stderr and **info and warnings to stdout**, and in the capture
//! worker stdout is the IPC pipe. The fork's `ipc-link` reads that pipe while
//! it waits for each reply and hands every line that is not JSON to
//! `log::info!("[rec]: ...")`, here in the daemon. So module loads, "not
//! loaded" warnings and the encoder libobs settles on all arrive as `log`
//! records, and until this existed they went nowhere.
//!
//! ## Where a record goes
//!
//! `route` decides; `BridgeLogger` only carries the answer out.
//!
//! - A `[rec]:` line from `ipc-link` is libobs's own output, so it goes into
//!   the libobs log file beside the errors the worker's stderr already writes
//!   there (`recorder::libobs::worker_log`), unformatted, so the file reads as
//!   one stream. Where that file does not exist (a `tauri:dev` build keeps
//!   the worker's stderr on the console) it comes to `daemon.log` instead,
//!   tagged `libobs`.
//! - Anything else from the capture crates comes to `daemon.log` at its own
//!   level, tagged `libobs`.
//! - Any other crate gets only warnings and errors through. Info from an HTTP
//!   or TLS stack is the kind of line that rotates a session's real errors
//!   out of the file, and debug from the capture crates is the same.

use crate::log::Level;

/// The crates whose info lines are worth a line in the log: the fork and the
/// two crates it is built from. Matched against the first segment of a
/// record's target, which is its crate.
const CAPTURE_CRATES: [&str; 3] = ["ipc_link", "libobs_recorder", "intprocess_recorder"];

/// What `ipc-link` puts in front of a line the worker wrote that was not a
/// reply. Spelled exactly as the fork formats it.
const WORKER_LINE_PREFIX: &str = "[rec]: ";

/// The tag everything from the capture crates carries in `daemon.log`. The
/// dev portal's Log panel already hides it by default, next to `live-poll`.
const CAPTURE_TAG: &str = "libobs";

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Route<'a> {
    /// A line of libobs's own output, with `ipc-link`'s prefix taken off.
    WorkerLine(&'a str),
    /// A line for `daemon.log`.
    App { level: Level, tag: &'a str, message: &'a str },
    Drop,
}

/// Where one record goes. Pure, so the whole policy is tested here rather
/// than by reading a file back.
pub(super) fn route<'a>(target: &'a str, level: ::log::Level, message: &'a str) -> Route<'a> {
    let crate_name = target.split("::").next().unwrap_or(target);
    let from_capture = CAPTURE_CRATES.contains(&crate_name);

    // The ceiling for this crate: info for the capture crates, warnings for
    // everyone else. `log::Level` orders Error < Warn < Info, so `>` reads
    // as "more verbose than".
    let ceiling = if from_capture { ::log::Level::Info } else { ::log::Level::Warn };
    if level > ceiling {
        return Route::Drop;
    }

    if crate_name == "ipc_link"
        && let Some(line) = message.strip_prefix(WORKER_LINE_PREFIX)
    {
        return Route::WorkerLine(line);
    }

    Route::App {
        level: to_level(level),
        tag: if from_capture { CAPTURE_TAG } else { crate_name },
        message,
    }
}

fn to_level(level: ::log::Level) -> Level {
    match level {
        ::log::Level::Error => Level::Error,
        ::log::Level::Warn => Level::Warn,
        ::log::Level::Info => Level::Info,
        ::log::Level::Debug | ::log::Level::Trace => Level::Debug,
    }
}

struct BridgeLogger;

impl ::log::Log for BridgeLogger {
    fn enabled(&self, metadata: &::log::Metadata) -> bool {
        // Nothing reaches `log` from here with an empty message, so a route
        // for an empty one answers the level question exactly.
        route(metadata.target(), metadata.level(), "") != Route::Drop
    }

    fn log(&self, record: &::log::Record) {
        // Formatted once, and only for a record that survives the level
        // check: `route` needs the text to see the `[rec]:` prefix.
        if !self.enabled(record.metadata()) {
            return;
        }
        let message = record.args().to_string();
        match route(record.target(), record.level(), &message) {
            Route::WorkerLine(line) => write_worker_line(line),
            Route::App { level, tag, message } => crate::log::write(level, tag, message),
            Route::Drop => {}
        }
    }

    fn flush(&self) {}
}

/// Into the libobs log file when there is one, so the worker's lines stay in
/// one place; into ours otherwise, rather than nowhere.
fn write_worker_line(line: &str) {
    #[cfg(target_os = "windows")]
    if crate::recorder::libobs::worker_log::append(line) {
        return;
    }
    crate::log::write(Level::Info, CAPTURE_TAG, line);
}

/// Installs the receiver. Once per process; a second call is a no-op, which
/// is what a test that starts a daemon twice needs.
pub(super) fn install() {
    if ::log::set_boxed_logger(Box::new(BridgeLogger)).is_ok() {
        // Info is the most any route lets through, so records below it are
        // skipped at the macro rather than formatted and then dropped.
        ::log::set_max_level(::log::LevelFilter::Info);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::log::Level as L;

    #[test]
    fn a_worker_line_goes_to_the_libobs_file_without_the_prefix() {
        assert_eq!(
            route("ipc_link", L::Info, "[rec]: info: Loading module: win-capture.dll"),
            Route::WorkerLine("info: Loading module: win-capture.dll")
        );
    }

    #[test]
    fn the_target_can_carry_a_module_path() {
        assert_eq!(
            route("ipc_link::inner", L::Info, "[rec]: warning: something"),
            Route::WorkerLine("warning: something")
        );
    }

    #[test]
    fn an_empty_worker_line_is_still_a_worker_line() {
        // libobs prints separator lines; the file should read as it would
        // on a console, blanks included.
        assert_eq!(route("ipc_link", L::Info, "[rec]: "), Route::WorkerLine(""));
    }

    #[test]
    fn only_ipc_link_can_produce_a_worker_line() {
        assert_eq!(
            route("libobs_recorder", L::Info, "[rec]: not really"),
            Route::App { level: Level::Info, tag: "libobs", message: "[rec]: not really" }
        );
    }

    #[test]
    fn other_capture_crate_records_come_to_our_log_at_their_level() {
        assert_eq!(
            route("intprocess_recorder::settings", L::Warn, "odd"),
            Route::App { level: Level::Warn, tag: "libobs", message: "odd" }
        );
        assert_eq!(
            route("ipc_link", L::Error, "no prefix"),
            Route::App { level: Level::Error, tag: "libobs", message: "no prefix" }
        );
    }

    #[test]
    fn debug_and_trace_are_dropped_even_from_the_capture_crates() {
        assert_eq!(route("ipc_link", L::Debug, "[rec]: x"), Route::Drop);
        assert_eq!(route("libobs_recorder", L::Trace, "x"), Route::Drop);
    }

    #[test]
    fn other_crates_get_warnings_and_errors_only() {
        assert_eq!(route("hyper::client", L::Info, "connected"), Route::Drop);
        assert_eq!(route("tungstenite::protocol", L::Debug, "frame"), Route::Drop);
        assert_eq!(
            route("rustls::conn", L::Warn, "bad cert"),
            Route::App { level: Level::Warn, tag: "rustls", message: "bad cert" }
        );
        assert_eq!(
            route("reqwest", L::Error, "boom"),
            Route::App { level: Level::Error, tag: "reqwest", message: "boom" }
        );
    }

    #[test]
    fn a_crate_that_only_shares_a_prefix_is_not_a_capture_crate() {
        assert_eq!(route("ipc_linker", L::Info, "[rec]: x"), Route::Drop);
    }

    #[test]
    fn levels_map_one_to_one_and_trace_folds_into_debug() {
        assert_eq!(to_level(L::Error), Level::Error);
        assert_eq!(to_level(L::Warn), Level::Warn);
        assert_eq!(to_level(L::Info), Level::Info);
        assert_eq!(to_level(L::Debug), Level::Debug);
        assert_eq!(to_level(L::Trace), Level::Debug);
    }
}
