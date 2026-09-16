//! UI-side helper: start the daemon if none is listening. WS3 task 3.5.
//!
//! The daemon is normally already running by the time the UI looks for it,
//! because the `Run` key starts one at login. This is the other path: a first
//! launch on a machine where autostart was never turned on, a daemon that
//! crashed, or a user who quit it and then opened the app. None of those should
//! be a dialog telling someone to go and start a background process.
//!
//! ## Probing is connecting
//!
//! There is no "is it running" call, and there should not be: any answer other
//! than an open connection is a guess that can be stale by the time it is
//! acted on. So the probe *is* `rpc::connect`, and its success is the thing the
//! caller wanted anyway. A daemon that answers is a daemon that is up, and the
//! connection is already made.
//!
//! ## Why this lives in `daemon/` rather than `ui/`
//!
//! The two sides have to agree on the address and the argument list. The
//! address they already share through `rpc::endpoint` and `rpc::connect`; this
//! file is the other half, and it sits beside them so a change to one is a
//! change in front of the other. `launch::DAEMON_FLAG` is the same agreement
//! written a third time, into the registry, by whichever build the user had
//! when they ticked the box.
//!
//! ## Nothing calls this yet
//!
//! The UI has no transport wired to its webview until the rest of WS3.4 lands,
//! so the only caller today is the tests, which drive it against a real
//! listener. Same treatment, and the same reason, as `ui::client`.

use std::io;
use std::path::Path;
use std::time::Duration;

use crate::daemon::rpc::{self, ClientStream};
use crate::launch::DAEMON_FLAG;
use crate::{info, warn};

/// How long to keep trying after asking for a daemon.
///
/// Generous because the first launch after an install is the slow one: a cold
/// binary, a database that may have migrations to run, and a folder scan before
/// the endpoint is answering. Being told "the recorder would not start" when it
/// was four seconds from ready would be the worse failure.
#[cfg_attr(not(test), allow(dead_code))]
const START_TIMEOUT: Duration = Duration::from_secs(10);

/// The gap between attempts while waiting for it to come up.
///
/// Flat rather than exponential, unlike `ui::client`'s reconnect backoff, and
/// the difference is deliberate: that loop runs forever against a daemon that
/// may never return, so it has to back off. This one runs against a process we
/// just started, for ten seconds, and the only thing a growing delay would buy
/// is a slower first launch.
const RETRY_EVERY: Duration = Duration::from_millis(100);

/// Connects to the daemon, starting one if nothing answers.
#[cfg_attr(not(test), allow(dead_code))]
pub async fn connect_or_start(endpoint: &Path) -> io::Result<impl ClientStream + use<>> {
    connect_or_start_with(endpoint, START_TIMEOUT, start_daemon).await
}

/// The testable half: the same logic, with the spawn and the deadline supplied.
///
/// A test cannot launch the real executable, because the thing running it is
/// `cargo test` rather than `ninja-recorder`. Injecting the spawn is what lets
/// the decision — probe, start, wait, give up — be exercised without one.
async fn connect_or_start_with<F>(
    endpoint: &Path,
    within: Duration,
    start: F,
) -> io::Result<impl ClientStream + use<F>>
where
    F: FnOnce() -> io::Result<()>,
{
    // The common case, and the only one that costs nothing: a daemon started at
    // login is already listening.
    if let Ok(stream) = rpc::connect(endpoint).await {
        return Ok(stream);
    }

    info!("daemon", "no daemon is listening on {}; starting one", endpoint.display());
    start()?;

    // `Instant` rather than a fixed attempt count: what matters is how long a
    // person has been waiting, and an attempt that takes a moment to fail
    // should not buy extra tries.
    let deadline = std::time::Instant::now() + within;
    let mut last: Option<io::Error> = None;
    while std::time::Instant::now() < deadline {
        match rpc::connect(endpoint).await {
            Ok(stream) => return Ok(stream),
            Err(e) => last = Some(e),
        }
        tokio::time::sleep(RETRY_EVERY).await;
    }

    // The daemon was asked for and never answered. Reported rather than
    // retried forever: something is wrong that waiting will not fix, and the
    // caller can say so while the reconnect loop keeps trying in the
    // background.
    let why = last.unwrap_or_else(|| io::Error::other("no connection attempt was made"));
    warn!("daemon", "a daemon was started but never answered: {why}");
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        format!("started a daemon but it did not answer within {within:?}: {why}"),
    ))
}

/// Launches this same executable with `--daemon`, detached.
///
/// One binary in two roles, so the path is our own
/// ([DEVELOPMENT.md §12](../../../DEVELOPMENT.md)). That also means the daemon
/// and the UI can never be different builds by accident, which is the failure
/// `hello`'s protocol check exists to catch and would rather not have to.
#[cfg_attr(not(test), allow(dead_code))]
fn start_daemon() -> io::Result<()> {
    let exe = std::env::current_exe()?;
    let mut command = std::process::Command::new(exe);
    command
        .arg(DAEMON_FLAG)
        // All three, because the child outlives us. A daemon holding the UI's
        // stdout keeps that pipe open after the UI is gone, and a daemon
        // writing to a console the UI owned writes into a window that is about
        // to close.
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        // `DETACHED_PROCESS`: the daemon gets no console at all, rather than
        // inheriting the one a `tauri dev` run is using. Spelled out for the
        // same reason `ffmpeg_command`'s `CREATE_NO_WINDOW` is — it lives
        // behind a `windows` crate feature this build does not otherwise need,
        // and the value is fixed ABI.
        //
        // Not `CREATE_NO_WINDOW`, which asks for a new console and then hides
        // it. The two are contradictory and the daemon wants neither: a
        // release build is a Windows-subsystem binary with no console to begin
        // with.
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        command.creation_flags(DETACHED_PROCESS);
    }

    // The handle is dropped on purpose: nothing here waits on the daemon, and
    // nothing should. On Unix that leaves a zombie entry until the UI exits,
    // which is a dev-box concern only — Windows has no such thing, and the
    // process this actually ships on is Windows.
    command.spawn()?;
    Ok(())
}

/// Starts the UI, from the daemon.
///
/// The other direction of this module: `connect_or_start` is the UI starting a
/// daemon, and this is the daemon's tray opening a window. Same executable,
/// same reasoning about which flags mean what, so it lives beside it rather
/// than in `pump`.
///
/// **No flag at all**, which is the whole argument: `Launch::Ui` is the default
/// and it is what creates a window. `--hidden` would start a UI with no window,
/// which is the opposite of what a person clicking Open wants.
///
/// Nothing here checks whether a UI is already running. The caller does, by
/// asking whether anything is subscribed to the daemon's events, because a UI
/// that is running is by definition connected.
pub fn start_ui() -> io::Result<()> {
    let exe = std::env::current_exe()?;
    let mut command = std::process::Command::new(exe);
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        // Same flag and the same reason as `start_daemon`: no console, rather
        // than a new hidden one. The UI is a Windows-subsystem binary in
        // release and has no console to begin with.
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        command.creation_flags(DETACHED_PROCESS);
    }

    command.spawn()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::rpc::Listener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// A unique endpoint per test, for the reason `rpc`'s own tests spell out:
    /// on Windows the namespace is shared with any daemon already running on
    /// the machine.
    fn test_endpoint() -> std::path::PathBuf {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let unique = format!("{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed));

        #[cfg(windows)]
        {
            std::path::PathBuf::from(format!(r"\\.\pipe\ninja-recorder-spawn-test.{unique}"))
        }
        #[cfg(unix)]
        {
            let dir = std::env::temp_dir().join(format!("nr-spawn-test-{unique}"));
            std::fs::create_dir_all(&dir).unwrap();
            dir.join("daemon.sock")
        }
    }

    /// The path taken at every login after the first: a daemon is already up,
    /// and starting a second would mean two processes racing for one recorder
    /// before the single-instance check sent one of them away.
    #[tokio::test]
    async fn a_daemon_that_answers_is_not_started_again() {
        let endpoint = test_endpoint();
        let _listener = Listener::bind(&endpoint).unwrap().expect("a fresh endpoint is free");

        let started = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&started);
        let stream = connect_or_start_with(&endpoint, Duration::from_secs(5), move || {
            counter.fetch_add(1, Ordering::Relaxed);
            Ok(())
        })
        .await;

        assert!(stream.is_ok(), "an answering daemon should just be connected to");
        assert_eq!(started.load(Ordering::Relaxed), 0, "nothing should have been started");
    }

    /// The recovery path: nothing is listening, so one is started and then
    /// waited for. The stand-in binds the endpoint from inside the spawn, which
    /// is what a real daemon does a moment after being launched.
    #[tokio::test]
    async fn nothing_listening_starts_one_and_waits_for_it() {
        let endpoint = test_endpoint();
        let bound = endpoint.clone();
        let held = Arc::new(std::sync::Mutex::new(None));
        let keep = Arc::clone(&held);

        let stream = connect_or_start_with(&endpoint, Duration::from_secs(5), move || {
            // Held for the length of the test: a listener dropped here would
            // take the endpoint away again before the retry could reach it.
            *keep.lock().unwrap() = Listener::bind(&bound)?;
            Ok(())
        })
        .await;

        assert!(stream.is_ok(), "the daemon came up and should have been connected to");
        assert!(held.lock().unwrap().is_some(), "the stand-in daemon bound the endpoint");
    }

    /// A spawn that cannot even be launched is reported straight away rather
    /// than waited out. The usual cause is an executable that has been moved or
    /// deleted, and ten seconds of retrying would not find it.
    #[tokio::test]
    async fn a_spawn_that_fails_is_reported_immediately() {
        let endpoint = test_endpoint();
        let started = std::time::Instant::now();

        let result = connect_or_start_with(&endpoint, Duration::from_secs(30), || {
            Err(io::Error::new(io::ErrorKind::NotFound, "no such executable"))
        })
        .await;

        let error = result.err().expect("a failed spawn is an error");
        assert_eq!(error.kind(), io::ErrorKind::NotFound, "the spawn's own error survives");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "a failed spawn must not wait out the timeout"
        );
    }

    /// And a daemon that was started but never came up gives up, rather than
    /// leaving the UI waiting on a process that is not going to answer.
    #[tokio::test]
    async fn a_daemon_that_never_answers_gives_up_and_says_so() {
        let endpoint = test_endpoint();

        let result =
            connect_or_start_with(&endpoint, Duration::from_millis(300), || Ok(())).await;

        let error = result.err().expect("nothing ever listened");
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(
            error.to_string().contains("did not answer"),
            "the error should say what happened: {error}"
        );
    }
}
