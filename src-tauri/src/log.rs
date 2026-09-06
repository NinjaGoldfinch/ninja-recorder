//! Where the app says what happened. DEVELOPMENT.md §13.
//!
//! ## Why this exists at all
//!
//! `main.rs` sets `windows_subsystem = "windows"` for release builds, so a
//! shipped app has **no console**. Every `eprintln!` in this crate was
//! therefore writing to a closed handle on the one machine where capture
//! problems actually happen — the dev portal's Log panel said as much in
//! its own header, and it only ever recorded the portal's own IPC calls.
//!
//! So: a file, written in release builds too, and one way to write to it.
//! The viewers stay behind `--features devtools`; the shipped surface is
//! this file and nothing else.
//!
//! ## Shape
//!
//! Formatting, level filtering and the rotation decision are pure
//! functions with direct unit tests. `Sink` is the thin I/O wrapper around
//! them (CLAUDE.md: pure decision, thin I/O wrapper).
//!
//! ## Not a logging crate
//!
//! `tracing` and `log` + `fern` both do this and more. What is needed here
//! is a timestamp, a level, a tag and a file that rotates — `tracing`'s
//! value is spans and structured fields, and nothing in this app has asked
//! for either. Revisit when something does.
//!
//! ## Nothing here may fail the app
//!
//! A read-only data dir, a locked file, a full disk: every one of those
//! degrades to "no file logging this session" and never to an error the
//! caller has to handle. Recording a game matters more than recording
//! *about* recording one — so `write` returns `()` and swallows I/O
//! errors, which is the one place in this codebase that is the right
//! trade.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// Rotate once the active file passes this. Three files at 5 MiB is about
/// two full play sessions of history, which is the window a capture bug is
/// actually diagnosed in.
const MAX_BYTES: u64 = 5 * 1024 * 1024;

/// The active file plus this many rotated ones.
const KEEP_ROTATED: usize = 2;

const FILE_STEM: &str = "ninja-recorder";

/// Severity, most severe first — the declaration order *is* the filter, so
/// `level <= max` reads as "at most this verbose".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Error,
    Warn,
    Info,
    Debug,
}

impl Level {
    /// Fixed width, so the tag column lines up when a human scans the file.
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Error => "ERROR",
            Level::Warn => "WARN ",
            Level::Info => "INFO ",
            Level::Debug => "DEBUG",
        }
    }

    fn from_env_value(value: &str) -> Option<Level> {
        match value.trim().to_ascii_lowercase().as_str() {
            "error" => Some(Level::Error),
            "warn" | "warning" => Some(Level::Warn),
            "info" => Some(Level::Info),
            "debug" | "trace" => Some(Level::Debug),
            _ => None,
        }
    }
}

/// `Info` by default: `Debug` is reserved for the high-volume streams
/// (libobs, per-poll tracking) that would otherwise rotate a session's
/// real errors straight out of the file.
static MAX_LEVEL: AtomicU8 = AtomicU8::new(Level::Info as u8);

static SINK: OnceLock<Mutex<Sink>> = OnceLock::new();

/// Opens the log file under `dir`, creating it if needed.
///
/// Call once, as early in `setup` as the app data dir is knowable — before
/// the database is opened, because failing to open the database is one of
/// the things worth having a log of.
///
/// Returns the active file's path when logging is live, so the caller can
/// say where it went. `None` means this session has no file and everything
/// falls back to stderr; that is not an error the caller should act on.
pub fn init(dir: &Path) -> Option<PathBuf> {
    if let Some(level) = std::env::var("NINJA_RECORDER_LOG_LEVEL")
        .ok()
        .and_then(|v| Level::from_env_value(&v))
    {
        set_max_level(level);
    }

    let sink = Sink::open(dir)?;
    let path = sink.path.clone();
    // Already initialized: a second call is a caller bug, not a reason to
    // lose the log we already have.
    if SINK.set(Mutex::new(sink)).is_err() {
        return None;
    }
    Some(path)
}

pub fn set_max_level(level: Level) {
    MAX_LEVEL.store(level as u8, Ordering::Relaxed);
}

pub fn max_level() -> Level {
    match MAX_LEVEL.load(Ordering::Relaxed) {
        0 => Level::Error,
        1 => Level::Warn,
        2 => Level::Info,
        _ => Level::Debug,
    }
}

/// The one write path. Called through the `error!`/`warn!`/`info!`/`debug!`
/// macros rather than directly.
pub fn write(level: Level, tag: &str, message: &str) {
    if level > max_level() {
        return;
    }
    let line = format_line(now_millis(), level, tag, message);

    // Keeps `npm run tauri:dev` behaving exactly as it did when all of
    // this was `eprintln!`. A release build has no console to write to.
    #[cfg(debug_assertions)]
    eprintln!("{line}");

    if let Some(sink) = SINK.get() {
        // A poisoned lock means another thread panicked mid-write. The log
        // is not worth propagating that into the app, so take the guard
        // anyway and carry on.
        let mut sink = match sink.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        sink.write_line(&line);
    }
}

/// One formatted line, without the trailing newline.
///
/// Pure so the format is pinned by a test rather than by reading the file
/// after the fact.
fn format_line(now_ms: i64, level: Level, tag: &str, message: &str) -> String {
    format!(
        "{} {} [{}] {}",
        timestamp(now_ms),
        level.as_str(),
        tag,
        message
    )
}

/// `2026-09-07T10:15:30.123Z`, hand-rolled because this crate has no date
/// dependency and this is the only thing that would justify one.
///
/// Milliseconds since the Unix epoch, UTC. Local time would be friendlier
/// to read and much worse to reason about across a DST boundary in the
/// middle of a play session.
fn timestamp(now_ms: i64) -> String {
    let (days, ms_of_day) = {
        let days = now_ms.div_euclid(86_400_000);
        let rem = now_ms.rem_euclid(86_400_000);
        (days, rem)
    };
    let (y, m, d) = civil_from_days(days);
    let (h, min, s, ms) = (
        ms_of_day / 3_600_000,
        (ms_of_day / 60_000) % 60,
        (ms_of_day / 1_000) % 60,
        ms_of_day % 1_000,
    );
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{min:02}:{s:02}.{ms:03}Z")
}

/// Days since the Unix epoch → (year, month, day). Howard Hinnant's
/// `civil_from_days`, which is the standard closed form and is exactly
/// what a date crate would run.
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Whether writing `incoming` more bytes should roll the file first.
///
/// Split out so the threshold is testable without writing five megabytes.
/// An empty file never rotates, however long the line — rotating to an
/// empty file and writing the same line into it would loop.
fn should_rotate(written: u64, incoming: u64) -> bool {
    written > 0 && written + incoming > MAX_BYTES
}

struct Sink {
    path: PathBuf,
    dir: PathBuf,
    file: Option<File>,
    written: u64,
}

impl Sink {
    fn open(dir: &Path) -> Option<Sink> {
        fs::create_dir_all(dir).ok()?;
        let path = dir.join(format!("{FILE_STEM}.log"));
        let file = OpenOptions::new().create(true).append(true).open(&path).ok()?;
        let written = file.metadata().map(|m| m.len()).unwrap_or(0);
        Some(Sink {
            path,
            dir: dir.to_path_buf(),
            file: Some(file),
            written,
        })
    }

    fn write_line(&mut self, line: &str) {
        let incoming = line.len() as u64 + 1;
        if should_rotate(self.written, incoming) {
            self.rotate();
        }
        let Some(file) = self.file.as_mut() else {
            return;
        };
        // A failed write drops the sink rather than retrying every line
        // for the rest of the session: the usual cause is a full disk or a
        // removed directory, and neither fixes itself.
        if writeln!(file, "{line}").is_err() {
            self.file = None;
            return;
        }
        self.written += incoming;
    }

    /// `x.log` → `x.1.log` → `x.2.log`, oldest dropped.
    ///
    /// Best-effort throughout: a rename that fails leaves the current file
    /// in place and logging continues into it, which is better than losing
    /// the sink over a transient lock.
    fn rotate(&mut self) {
        self.file = None;

        for index in (1..=KEEP_ROTATED).rev() {
            let from = if index == 1 {
                self.path.clone()
            } else {
                self.dir.join(format!("{FILE_STEM}.{}.log", index - 1))
            };
            let to = self.dir.join(format!("{FILE_STEM}.{index}.log"));
            if index == KEEP_ROTATED {
                let _ = fs::remove_file(&to);
            }
            let _ = fs::rename(&from, &to);
        }

        self.file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .ok();
        self.written = 0;
    }
}

/// Something went wrong and the user may notice.
#[macro_export]
macro_rules! error {
    ($tag:expr, $($arg:tt)*) => {
        $crate::log::write($crate::log::Level::Error, $tag, &format!($($arg)*))
    };
}

/// Something went wrong and the app carried on regardless.
#[macro_export]
macro_rules! warn {
    ($tag:expr, $($arg:tt)*) => {
        $crate::log::write($crate::log::Level::Warn, $tag, &format!($($arg)*))
    };
}

/// A thing happened that a human reading the log would want confirmed.
#[macro_export]
macro_rules! info {
    ($tag:expr, $($arg:tt)*) => {
        $crate::log::write($crate::log::Level::Info, $tag, &format!($($arg)*))
    };
}

/// Off by default. For the high-volume streams — libobs, per-poll
/// tracking — that would otherwise rotate real errors out of the file.
#[macro_export]
macro_rules! debug {
    ($tag:expr, $($arg:tt)*) => {
        $crate::log::write($crate::log::Level::Debug, $tag, &format!($($arg)*))
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- the line ---------------------------------------------------------

    #[test]
    fn a_line_carries_a_timestamp_a_level_and_the_tag() {
        assert_eq!(
            format_line(0, Level::Error, "state_machine", "failed to start"),
            "1970-01-01T00:00:00.000Z ERROR [state_machine] failed to start"
        );
    }

    /// The tag column only lines up if the level does, and a log nobody can
    /// scan is a log nobody reads.
    #[test]
    fn levels_are_padded_to_one_width() {
        let widths: Vec<usize> = [Level::Error, Level::Warn, Level::Info, Level::Debug]
            .iter()
            .map(|l| l.as_str().len())
            .collect();
        assert_eq!(widths, vec![5, 5, 5, 5]);
    }

    // --- the timestamp ----------------------------------------------------

    #[test]
    fn the_epoch_formats_as_the_epoch() {
        assert_eq!(timestamp(0), "1970-01-01T00:00:00.000Z");
    }

    #[test]
    fn a_known_instant_round_trips() {
        // 2026-09-07T10:15:30.123Z, cross-checked against `date -u -d @...`.
        assert_eq!(timestamp(1_788_776_130_123), "2026-09-07T10:15:30.123Z");
    }

    /// The one case a hand-rolled calendar gets wrong. 2024 is a leap year,
    /// so the 60th day is the 29th of February and not the 1st of March.
    #[test]
    fn a_leap_day_is_a_leap_day() {
        // 2024-02-29T12:00:00.000Z
        assert_eq!(timestamp(1_709_208_000_000), "2024-02-29T12:00:00.000Z");
    }

    /// 2000 is a leap year and 1900 was not — the rule that catches out
    /// every naive implementation.
    #[test]
    fn the_century_rule_holds() {
        // 2000-02-29T00:00:00.000Z
        assert_eq!(timestamp(951_782_400_000), "2000-02-29T00:00:00.000Z");
    }

    #[test]
    fn a_time_before_the_epoch_does_not_wrap_into_nonsense() {
        // 1969-12-31T23:59:59.000Z — div_euclid, not integer division.
        assert_eq!(timestamp(-1_000), "1969-12-31T23:59:59.000Z");
    }

    // --- filtering --------------------------------------------------------

    #[test]
    fn severity_orders_most_severe_first() {
        assert!(Level::Error < Level::Warn);
        assert!(Level::Warn < Level::Info);
        assert!(Level::Info < Level::Debug);
    }

    #[test]
    fn the_env_override_takes_the_names_a_person_would_type() {
        assert_eq!(Level::from_env_value("debug"), Some(Level::Debug));
        assert_eq!(Level::from_env_value("  WARN "), Some(Level::Warn));
        assert_eq!(Level::from_env_value("warning"), Some(Level::Warn));
        assert_eq!(Level::from_env_value("shout"), None);
    }

    // --- rotation ---------------------------------------------------------

    #[test]
    fn rotation_waits_until_the_threshold_is_actually_passed() {
        assert!(!should_rotate(0, 100));
        assert!(!should_rotate(MAX_BYTES - 10, 5));
        assert!(should_rotate(MAX_BYTES - 10, 20));
    }

    /// Otherwise a line longer than the whole budget would rotate to an
    /// empty file, fail to fit, and rotate again on the next line —
    /// throwing away the history it was meant to protect.
    #[test]
    fn an_empty_file_never_rotates_however_long_the_line() {
        assert!(!should_rotate(0, MAX_BYTES * 10));
    }

    // --- the sink ---------------------------------------------------------

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ninja-recorder-log-{}-{}-{name}",
            std::process::id(),
            now_millis()
        ));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn lines_land_in_the_file() {
        let dir = temp_dir("writes");
        let mut sink = Sink::open(&dir).expect("a fresh temp dir is writable");
        sink.write_line("first");
        sink.write_line("second");
        drop(sink);

        let body = fs::read_to_string(dir.join("ninja-recorder.log")).unwrap();
        assert_eq!(body, "first\nsecond\n");
        let _ = fs::remove_dir_all(&dir);
    }

    /// Reopening appends rather than truncating — a restart after a crash
    /// must not destroy the log of the crash.
    #[test]
    fn reopening_keeps_what_was_already_there() {
        let dir = temp_dir("append");
        let mut sink = Sink::open(&dir).unwrap();
        sink.write_line("before the restart");
        drop(sink);

        let mut sink = Sink::open(&dir).unwrap();
        sink.write_line("after it");
        drop(sink);

        let body = fs::read_to_string(dir.join("ninja-recorder.log")).unwrap();
        assert_eq!(body, "before the restart\nafter it\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rotating_rolls_the_file_and_keeps_the_cap() {
        let dir = temp_dir("rotate");
        let mut sink = Sink::open(&dir).unwrap();

        sink.write_line("oldest");
        sink.rotate();
        sink.write_line("middle");
        sink.rotate();
        sink.write_line("newest");
        drop(sink);

        assert_eq!(
            fs::read_to_string(dir.join("ninja-recorder.log")).unwrap(),
            "newest\n"
        );
        assert_eq!(
            fs::read_to_string(dir.join("ninja-recorder.1.log")).unwrap(),
            "middle\n"
        );
        assert_eq!(
            fs::read_to_string(dir.join("ninja-recorder.2.log")).unwrap(),
            "oldest\n"
        );

        // And the oldest falls off rather than accumulating forever.
        let mut sink = Sink::open(&dir).unwrap();
        sink.write_line("newer still");
        sink.rotate();
        drop(sink);
        assert!(!dir.join("ninja-recorder.3.log").exists());
        assert_eq!(
            fs::read_to_string(dir.join("ninja-recorder.2.log")).unwrap(),
            "middle\n",
            "the oldest file should have been dropped, not shuffled"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    /// The whole failure posture in one test: a path that cannot be opened
    /// yields no sink, and the caller carries on without a log rather than
    /// getting an error it has to handle.
    #[test]
    fn an_unwritable_destination_degrades_instead_of_failing() {
        let dir = temp_dir("unwritable");
        fs::create_dir_all(&dir).unwrap();
        // A file where the directory should be: `create_dir_all` cannot
        // succeed, so `open` must give up rather than panic.
        let blocked = dir.join("blocked");
        fs::write(&blocked, b"not a directory").unwrap();

        assert!(Sink::open(&blocked).is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    /// The global write path, start to finish — and the only test that
    /// touches the process-wide sink, because `init` is a `OnceLock` and a
    /// second test racing it would be flaky rather than wrong.
    ///
    /// Both halves matter: logging *before* `init` must be a silent no-op
    /// rather than a panic (anything failing during early startup hits
    /// that), and logging after it must actually reach the file.
    #[test]
    fn the_global_writer_is_silent_before_init_and_writes_after_it() {
        write(Level::Error, "test", "nobody is listening yet");

        let dir = temp_dir("global");
        let path = init(&dir).expect("a fresh temp dir is writable");
        assert_eq!(path, dir.join("ninja-recorder.log"));

        write(Level::Error, "state_machine", "failed to stop recording");
        // Filtered out: `Debug` is off unless something asks for it.
        write(Level::Debug, "libobs", "a very chatty line");

        let body = fs::read_to_string(&path).unwrap();
        assert!(
            body.contains("ERROR [state_machine] failed to stop recording"),
            "{body}"
        );
        assert!(!body.contains("nobody is listening yet"), "{body}");
        assert!(!body.contains("a very chatty line"), "{body}");

        // And a line that was filtered arrives once the level allows it.
        set_max_level(Level::Debug);
        write(Level::Debug, "libobs", "a very chatty line");
        set_max_level(Level::Info);
        assert!(fs::read_to_string(&path).unwrap().contains("a very chatty line"));

        let _ = fs::remove_dir_all(&dir);
    }
}
