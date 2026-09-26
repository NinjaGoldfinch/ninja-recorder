//! The daemon's end of the capture worker: the child process, the job object
//! that ties its life to the daemon's, and the two pipes.
//!
//! Every way a call can fail ends the same way: the worker is killed if it is
//! still there, reaped, and described with its exit code. A caller that gets
//! an `Err` from [`Worker::ask`] holds a worker that is gone, and drops it.
//! So a worker that crashed, hung, or started talking nonsense costs the
//! daemon a log line and a fresh spawn, never the daemon itself.
//!
//! Replies are read on a thread of their own and handed over on a channel,
//! which is what lets every call have a timeout: a worker wedged inside a
//! driver would otherwise hold the recorder lock, and with it the
//! supervisor, for as long as the driver liked.
//!
//! **A worker that dies is known at once, not at the next call** (#299). The
//! reply thread reaches EOF the moment the worker's end of its stdout closes,
//! which it only does by exiting, however that happens: killed in Task
//! Manager, a driver fault, a panic. It marks the worker closed and calls
//! the `on_close` it was spawned with, and [`Worker::gone`] reads the mark.
//! So the daemon can finish a recording whose worker has gone while the game
//! is still running, instead of discovering it at the stop.
//!
//! The worker's stderr is drained into `daemon.log` on another thread. In a
//! release build nothing writes there but a panic's report and anything that
//! tried to print to stdout (`worker::take_stdout`), which are exactly the
//! lines worth having beside the daemon's own. Everything the worker logs on
//! purpose goes to `worker.log`, which is why `collect_output` has nothing to
//! do for this backend.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, Instant};

use super::protocol::{self, Line, PROTOCOL_VERSION, Reply, Request};
use crate::recorder::own::select;
use crate::warn;

/// How long a fresh worker has to answer the handshake. It has loaded the
/// binary and done nothing else, so this is generous.
pub const HELLO_WAIT: Duration = Duration::from_secs(10);

/// How long a worker that has closed its stdout, or been told to go, is
/// given to exit on its own before it is killed.
const EXIT_GRACE: Duration = Duration::from_secs(2);

/// Called once, on the reply thread, when the worker's stdout reaches EOF:
/// the worker has exited, or is exiting. Must return at once; it runs on the
/// thread that reads the worker's replies.
pub type OnClose = Box<dyn FnOnce() + Send>;

/// A running capture worker.
pub struct Worker {
    child: Child,
    /// `None` once closed, which the worker reads as EOF.
    stdin: Option<ChildStdin>,
    replies: Receiver<Line<Reply>>,
    /// Kills the worker if the daemon dies without saying so.
    #[cfg(target_os = "windows")]
    _job: Option<super::job::Job>,
    /// Set once the process has been waited for.
    exited: Option<ExitStatus>,
    /// Set by the reply thread when the worker's stdout reaches EOF.
    closed: Arc<AtomicBool>,
}

impl Worker {
    /// Spawns `exe --capture-worker` and completes the handshake. The worker
    /// writes `worker.log` into `log_dir`, if there is one, and `on_close` is
    /// called when its stdout closes (see the module comment).
    pub fn spawn(
        exe: &Path,
        log_dir: Option<&Path>,
        on_close: Option<OnClose>,
    ) -> Result<Worker, String> {
        let mut command = Command::new(exe);
        command.arg(crate::launch::CAPTURE_WORKER_FLAG);
        if let Some(dir) = log_dir {
            command.env(super::LOG_DIR_ENV, dir);
        }
        let forced = select::software_forced_in_this_build(|name| std::env::var(name).ok());
        if forced {
            warn!(
                "recorder",
                "{}=1: the capture worker will encode in SOFTWARE, for testing only",
                select::FORCE_SOFTWARE_ENV
            );
        }
        pass_software_override(&mut command, forced);
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            // The same flag `ffmpeg_command` spawns with, spelled out for the
            // same reason. A release build is a Windows-subsystem binary with
            // no console anyway; a debug build is a console one, and this
            // keeps it from opening a window of its own.
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        Self::spawn_command(command, on_close)
    }

    /// Spawns whatever `command` names as a worker. Split out so the tests
    /// can stand a script in for the real one.
    pub(crate) fn spawn_command(
        mut command: Command,
        on_close: Option<OnClose>,
    ) -> Result<Worker, String> {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("could not start the capture worker: {e}"))?;
        let pid = child.id();

        // Into the job before anything else: from here on, a daemon that dies
        // takes the worker with it. There is a window between the spawn and
        // this in which it would not, and EOF on the worker's stdin covers
        // it: a dead daemon's end of the pipe closes, and the worker finalizes
        // and exits.
        #[cfg(target_os = "windows")]
        let job = match super::job::Job::contain(&child) {
            Ok(job) => Some(job),
            Err(e) => {
                warn!(
                    "recorder",
                    "capture worker pid {pid} is not in a job object ({e}); it will still \
                     exit when the daemon's end of its stdin closes"
                );
                None
            }
        };

        let stdout = child.stdout.take().expect("piped above");
        let (sender, replies) = channel();
        let closed = Arc::new(AtomicBool::new(false));
        let reader = std::thread::Builder::new()
            .name("capture-worker-replies".to_string())
            .spawn({
                let closed = Arc::clone(&closed);
                move || {
                    let mut stdout = BufReader::new(stdout);
                    loop {
                        match protocol::read_line::<Reply>(&mut stdout) {
                            // The worker has gone, or is going: said at once,
                            // not at the next call (#299). Dropping `sender`
                            // on the way out is what a call waiting for a
                            // reply sees.
                            Line::Eof => {
                                closed.store(true, Ordering::Release);
                                if let Some(on_close) = on_close {
                                    on_close();
                                }
                                return;
                            }
                            // Nobody is listening: the `Worker` was dropped.
                            line => {
                                if sender.send(line).is_err() {
                                    return;
                                }
                            }
                        }
                    }
                }
            });
        if let Some(stderr) = child.stderr.take() {
            let _ = std::thread::Builder::new()
                .name("capture-worker-stderr".to_string())
                .spawn(move || {
                    for line in BufReader::new(stderr).lines() {
                        let Ok(line) = line else { return };
                        if !line.trim().is_empty() {
                            warn!("capture-worker", "pid {pid} stderr: {line}");
                        }
                    }
                });
        }

        let mut worker = Worker {
            stdin: child.stdin.take(),
            child,
            replies,
            #[cfg(target_os = "windows")]
            _job: job,
            exited: None,
            closed,
        };
        if let Err(e) = reader {
            return Err(worker.bury(&format!("could not start its reader thread: {e}")));
        }
        match worker.ask(&Request::Hello { protocol: PROTOCOL_VERSION }, HELLO_WAIT)? {
            Reply::Hello { protocol, .. } if protocol == PROTOCOL_VERSION => Ok(worker),
            other => Err(worker.bury(&format!("answered the handshake with {other:?}"))),
        }
    }

    /// The worker's process id.
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Sends `request` and waits up to `timeout` for the answer. An `Err`
    /// means the worker is gone (it has been killed and reaped, and the
    /// message says how it ended); drop it.
    pub fn ask(&mut self, request: &Request, timeout: Duration) -> Result<Reply, String> {
        self.tell(request)?;
        match self.replies.recv_timeout(timeout) {
            Ok(Line::Message(Reply::Refused { reason })) => {
                Err(self.bury(&format!("refused {request:?}: {reason}")))
            }
            Ok(Line::Message(reply)) => Ok(reply),
            Ok(Line::Garbled(line)) => Err(self.bury(&format!("wrote a non-message: {line}"))),
            Ok(Line::Eof) | Err(RecvTimeoutError::Disconnected) => {
                Err(self.bury(&format!("closed its pipe during {request:?}")))
            }
            Err(RecvTimeoutError::Timeout) => {
                Err(self.bury(&format!("did not answer {request:?} within {timeout:?}")))
            }
        }
    }

    /// Sends a request that has no answer.
    pub fn tell(&mut self, request: &Request) -> Result<(), String> {
        let written = match self.stdin.as_mut() {
            Some(stdin) => protocol::write_line(stdin, request),
            None => Err(std::io::ErrorKind::BrokenPipe.into()),
        };
        written.map_err(|e| self.bury(&format!("could not be sent {request:?}: {e}")))
    }

    /// Whether the process is still running. A worker found dead here has
    /// been reaped, and `Some` carries how it ended, for the log.
    pub fn exited(&mut self) -> Option<String> {
        if self.exited.is_none() {
            self.exited = self.child.try_wait().ok().flatten();
        }
        self.exited.map(|status| format!("capture worker pid {} {}", self.pid(), ended(status)))
    }

    /// Whether the worker has gone: its process has exited, or it has closed
    /// its stdout, which it only does by exiting. Unlike [`Worker::exited`]
    /// this does not wait for the process to be reapable, so it answers as
    /// soon as the reply thread has seen the EOF; a worker found gone that
    /// way is reaped here (after a short grace, killed if it lingers), and
    /// `Some` says how it ended, with its exit code, for the log and the
    /// recording's problem.
    pub fn gone(&mut self) -> Option<String> {
        if let Some(why) = self.exited() {
            return Some(why);
        }
        self.closed
            .load(Ordering::Acquire)
            .then(|| self.bury("closed its pipe mid-recording"))
    }

    /// Asks the worker to exit and waits up to `timeout` for it, finalizing
    /// anything in flight on the way; kills it if it takes longer. Returns
    /// how it ended.
    pub fn shut_down(mut self, timeout: Duration) -> String {
        self.finish(timeout)
    }

    fn finish(&mut self, timeout: Duration) -> String {
        if self.exited.is_none() {
            if let Some(mut stdin) = self.stdin.take() {
                // `Release`, and then EOF behind it: either one ends the loop.
                let _ = protocol::write_line(&mut stdin, &Request::Release);
            }
            self.wait_or_kill(timeout);
        }
        match self.exited {
            Some(status) => format!("capture worker pid {} {}", self.pid(), ended(status)),
            None => format!("capture worker pid {} could not be waited for", self.pid()),
        }
    }

    /// Ends a worker that failed a call, and says how it ended: killed if it
    /// was still running after a short grace, then reaped.
    fn bury(&mut self, what: &str) -> String {
        self.stdin = None;
        if self.exited.is_none() {
            self.wait_or_kill(EXIT_GRACE);
        }
        let how = self.exited.map_or_else(|| "could not be waited for".to_string(), ended);
        format!("capture worker pid {} {what}; it {how}", self.pid())
    }

    fn wait_or_kill(&mut self, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => {
                    self.exited = Some(status);
                    return;
                }
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                _ => break,
            }
        }
        let _ = self.child.kill();
        self.exited = self.child.wait().ok();
    }
}

impl Drop for Worker {
    /// A worker is never simply abandoned: dropped without `shut_down`, it
    /// is told to go and killed if it does not.
    fn drop(&mut self) {
        if self.exited.is_none() {
            self.finish(EXIT_GRACE);
        }
    }
}

/// Hands the daemon's decision about the software-encoder override
/// (`select::FORCE_SOFTWARE_ENV`) to the worker explicitly, rather than
/// leaving it to whatever environment the worker happens to inherit: set to
/// `1` when the daemon honours it, removed when it does not. So a release
/// build's worker never sees the variable at all, and a devtools worker sees
/// it exactly when its daemon logged that it would. The worker checks it
/// again with the same gate before acting on it.
fn pass_software_override(command: &mut Command, forced: bool) {
    if forced {
        command.env(select::FORCE_SOFTWARE_ENV, "1");
    } else {
        command.env_remove(select::FORCE_SOFTWARE_ENV);
    }
}

/// `exited with code 3`, `exited with exit code: 0xc0000005`, `was killed by
/// signal 9`: whatever the platform can say.
fn ended(status: ExitStatus) -> String {
    match status.code() {
        Some(0) => "exited cleanly (code 0)".to_string(),
        Some(code) => format!("exited with code {code} ({status})"),
        None => format!("ended without an exit code ({status})"),
    }
}

/// Driven by shell scripts standing in for the worker, so they run where
/// there is a `sh`. The real worker is exercised by `tests/capture_worker.rs`.
#[cfg(all(test, unix))]
mod tests {
    use super::*;

    const HELLO: &str = r#"{"type":"hello","protocol":1,"pid":1,"version":"x"}"#;

    fn fake(script: &str) -> Command {
        let mut command = Command::new("sh");
        command.args(["-c", script]);
        command
    }

    fn spawn(script: &str) -> Result<Worker, String> {
        Worker::spawn_command(fake(script), None)
    }

    #[test]
    fn the_handshake_then_a_clean_shutdown() {
        let script = format!("read l; echo '{HELLO}'; read l; exit 0");
        let worker = spawn(&script).expect("handshake");
        assert!(worker.shut_down(Duration::from_secs(5)).contains("exited cleanly"));
    }

    #[test]
    fn a_wrong_protocol_is_refused_at_spawn() {
        let script = r#"read l; echo '{"type":"hello","protocol":99,"pid":1,"version":"x"}'"#;
        let Err(why) = spawn(script) else { panic!("accepted") };
        assert!(why.contains("handshake"), "{why}");
    }

    /// The worker dies mid-call: the caller gets an error naming the exit
    /// code, and the daemon carries on.
    #[test]
    fn a_worker_that_dies_is_reported_with_its_exit_code() {
        let script = format!("read l; echo '{HELLO}'; read l; exit 7");
        let mut worker = spawn(&script).expect("handshake");
        let why = worker.ask(&Request::Stop, Duration::from_secs(5)).unwrap_err();
        assert!(why.contains("closed its pipe"), "{why}");
        assert!(why.contains("code 7"), "{why}");
        assert!(worker.exited().is_some());
    }

    /// A wedged worker is killed at the timeout rather than waited on.
    #[test]
    fn a_worker_that_hangs_is_killed_at_the_timeout() {
        let script = format!("read l; echo '{HELLO}'; read l; sleep 30");
        let mut worker = spawn(&script).expect("handshake");
        let started = Instant::now();
        let why = worker.ask(&Request::Prepare, Duration::from_millis(200)).unwrap_err();
        assert!(why.contains("did not answer"), "{why}");
        assert!(started.elapsed() < Duration::from_secs(10), "waited for the sleep");
    }

    /// A worker that died between calls is noticed before the next one.
    #[test]
    fn a_dead_worker_is_noticed_between_calls() {
        let script = format!("read l; echo '{HELLO}'; exit 3");
        let mut worker = spawn(&script).expect("handshake");
        let deadline = Instant::now() + Duration::from_secs(5);
        let why = loop {
            if let Some(why) = worker.exited() {
                break why;
            }
            assert!(Instant::now() < deadline, "never exited");
            std::thread::sleep(Duration::from_millis(20));
        };
        assert!(why.contains("code 3"), "{why}");
    }

    /// A worker that dies while nothing is asking it anything (killed in Task
    /// Manager mid-game) is announced at once by `on_close`, and `gone` then
    /// says how it ended, without waiting for a call (#299).
    #[test]
    fn a_worker_that_dies_between_calls_is_announced_at_once() {
        let (told, heard) = channel();
        let on_close: OnClose = Box::new(move || {
            let _ = told.send(());
        });
        let script = format!("read l; echo '{HELLO}'; sleep 0.2; exit 1");
        let mut worker = Worker::spawn_command(fake(&script), Some(on_close)).expect("handshake");
        assert_eq!(worker.gone(), None, "gone before it exited");
        heard.recv_timeout(Duration::from_secs(5)).expect("on_close was never called");
        let why = worker.gone().expect("closed, so gone");
        assert!(why.contains("code 1"), "{why}");
    }

    /// A worker that is up and quiet is not gone, and one released on
    /// purpose closes its pipe too: `on_close` fires for every end, and it is
    /// the recorder, asked afterwards, that decides whether it mattered.
    #[test]
    fn a_quiet_worker_is_not_gone_and_a_release_still_closes() {
        let (told, heard) = channel();
        let on_close: OnClose = Box::new(move || {
            let _ = told.send(());
        });
        let script = format!("read l; echo '{HELLO}'; read l; exit 0");
        let mut worker = Worker::spawn_command(fake(&script), Some(on_close)).expect("handshake");
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(worker.gone(), None);
        assert!(heard.try_recv().is_err(), "announced a worker that is still up");
        assert!(worker.shut_down(Duration::from_secs(5)).contains("exited cleanly"));
        heard.recv_timeout(Duration::from_secs(5)).expect("on_close after the release");
    }

    #[test]
    fn a_worker_that_cannot_start_is_an_error() {
        let Err(why) = Worker::spawn_command(Command::new("/nonexistent/worker"), None) else {
            panic!("spawned");
        };
        assert!(why.contains("could not start"), "{why}");
    }

    /// A stand-in worker that completes the handshake only if it was started
    /// with the software override set to exactly `1`, and exits 5 otherwise.
    fn needs_override() -> Command {
        let var = select::FORCE_SOFTWARE_ENV;
        fake(&format!(r#"read l; [ "${{{var}:-unset}}" = 1 ] || exit 5; echo '{HELLO}'; read l"#))
    }

    /// The daemon's decision reaches the worker process: set when honoured,
    /// even with nothing in the daemon's own environment to inherit.
    #[test]
    fn a_forced_software_encoder_reaches_the_worker() {
        let mut command = needs_override();
        pass_software_override(&mut command, true);
        let worker = Worker::spawn_command(command, None).expect("the worker saw the override");
        assert!(worker.shut_down(Duration::from_secs(5)).contains("exited cleanly"));
    }

    /// And removed when not, so a worker cannot pick up a value its daemon
    /// declined, whatever the environment it would have inherited says.
    #[test]
    fn an_override_the_daemon_declined_never_reaches_the_worker() {
        let mut command = needs_override();
        command.env(select::FORCE_SOFTWARE_ENV, "1"); // as if inherited
        pass_software_override(&mut command, false);
        let Err(why) = Worker::spawn_command(command, None) else {
            panic!("the worker saw the override")
        };
        assert!(why.contains("code 5"), "{why}");
    }
}
