//! Whether a file is still open in another process, for startup recovery.
//!
//! Recovery rewrites an interrupted recording: it cuts a torn tail, appends
//! an index, and replaces the file with a remuxed copy. All of that assumes
//! the writer is dead. A capture worker that outlived its daemon is not, and
//! on the box one was still recording when the next daemon's recovery cut
//! 4.36 MB of live footage off the end and then failed its own replace
//! (#307). Both backends' workers now die with the daemon
//! (`recorder::job`), but a worker is torn down asynchronously and an older
//! build's worker is in no job at all, so recovery asks first.
//!
//! **The question is an exclusive open.** On Windows a file opened with a
//! share mode of 0 fails with `ERROR_SHARING_VIOLATION` while any other handle
//! to it is open, which is exactly "something is still writing this". Other
//! failures (a read-only file, one that has gone) are not answers to that
//! question, and are left to the step that would have failed on them anyway.
//! Elsewhere there is no capture backend to leave a writer behind, and the
//! check reports the file free.
//!
//! [`open_says_free`] and [`wait_until_free`] are the decisions, pure; [`is_free`]
//! is the open.

use std::path::Path;
use std::time::Duration;

/// How long recovery waits for a writer to let go of a file before leaving
/// the recording for a later start. A worker in a closing job is gone in
/// milliseconds; this is for the one that is not, and it is paid at most once
/// per interrupted recording, before the daemon can record.
pub const WAIT: Duration = Duration::from_secs(10);
/// How often the file is asked again during [`WAIT`].
pub const INTERVAL: Duration = Duration::from_millis(250);

/// `ERROR_SHARING_VIOLATION`: another handle's share mode refuses ours.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const ERROR_SHARING_VIOLATION: i32 = 32;
/// `ERROR_LOCK_VIOLATION`: another process has a byte range locked.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const ERROR_LOCK_VIOLATION: i32 = 33;

/// Whether an exclusive open's result says the file is free, from its raw
/// Windows error code (`None` for a successful open).
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub fn open_says_free(raw_os_error: Option<i32>) -> bool {
    !matches!(raw_os_error, Some(ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION))
}

/// Asks `probe` whether the file is free, up to `attempts` times, calling
/// `sleep(interval)` between asks. `true` as soon as it is.
pub fn wait_until_free(
    mut probe: impl FnMut() -> bool,
    mut sleep: impl FnMut(Duration),
    attempts: u32,
    interval: Duration,
) -> bool {
    for attempt in 0..attempts.max(1) {
        if probe() {
            return true;
        }
        if attempt + 1 < attempts {
            sleep(interval);
        }
    }
    false
}

/// Whether nothing else has `path` open: an open with write access and no
/// sharing at all succeeds only then. The handle is closed at once.
#[cfg(target_os = "windows")]
pub fn is_free(path: &Path) -> bool {
    use std::os::windows::fs::OpenOptionsExt;
    let open = std::fs::OpenOptions::new().read(true).write(true).share_mode(0).open(path);
    open_says_free(open.err().and_then(|e| e.raw_os_error()))
}

/// No capture backend runs here, so nothing can be left writing.
#[cfg(not(target_os = "windows"))]
pub fn is_free(_path: &Path) -> bool {
    true
}

/// [`is_free`], retried for up to [`WAIT`].
pub fn wait_for_writer(path: &Path) -> bool {
    let attempts = (WAIT.as_millis() / INTERVAL.as_millis()) as u32;
    wait_until_free(|| is_free(path), std::thread::sleep, attempts, INTERVAL)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sharing_or_lock_violation_is_in_use() {
        assert!(!open_says_free(Some(32)));
        assert!(!open_says_free(Some(33)));
    }

    /// Anything else is not an answer to "is it being written", so recovery
    /// goes ahead and the step that would fail anyway reports it.
    #[test]
    fn success_and_every_other_error_are_free() {
        assert!(open_says_free(None));
        assert!(open_says_free(Some(2)), "not found");
        assert!(open_says_free(Some(5)), "access denied: a read-only file");
    }

    #[test]
    fn a_free_file_is_not_waited_on() {
        let mut slept = Vec::new();
        assert!(wait_until_free(|| true, |d| slept.push(d), 40, INTERVAL));
        assert!(slept.is_empty());
    }

    #[test]
    fn a_writer_that_lets_go_is_waited_for() {
        let mut asks = 0;
        let mut slept = Vec::new();
        let free = wait_until_free(
            || {
                asks += 1;
                asks == 3
            },
            |d| slept.push(d),
            40,
            INTERVAL,
        );
        assert!(free);
        assert_eq!(asks, 3);
        assert_eq!(slept, [INTERVAL, INTERVAL]);
    }

    /// Bounded: a writer that never lets go costs the budget and no more,
    /// with no sleep after the last ask.
    #[test]
    fn a_writer_that_never_lets_go_is_given_up_on() {
        let mut asks = 0;
        let mut slept = Duration::ZERO;
        let free = wait_until_free(
            || {
                asks += 1;
                false
            },
            |d| slept += d,
            40,
            INTERVAL,
        );
        assert!(!free);
        assert_eq!(asks, 40);
        assert_eq!(slept, INTERVAL * 39);
        assert!(slept < WAIT);
    }

    #[test]
    fn a_file_nobody_has_open_is_free() {
        let path = std::env::temp_dir()
            .join(format!("ninja-recorder-in-use-{}", std::process::id()));
        std::fs::write(&path, b"x").unwrap();
        assert!(is_free(&path));
        std::fs::remove_file(&path).ok();
    }

    /// The case #307 is about, on the platform it happens on: a handle open
    /// for writing elsewhere makes the file in use until it closes.
    #[cfg(target_os = "windows")]
    #[test]
    fn a_file_open_for_writing_is_in_use_until_it_closes() {
        let path = std::env::temp_dir()
            .join(format!("ninja-recorder-in-use-open-{}", std::process::id()));
        let writer = std::fs::File::create(&path).unwrap();
        assert!(!is_free(&path));
        drop(writer);
        assert!(is_free(&path));
        std::fs::remove_file(&path).ok();
    }
}
