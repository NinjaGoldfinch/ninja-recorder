//! The capture worker's loop: read a request, ask the session, write the
//! answer, until `Release` or the end of stdin.
//!
//! Generic over the reader, the writer and the [`Host`] that stands in for
//! the session thread, so the loop, and above all its handling of EOF, is
//! tested on any host with bytes in memory. On Windows the host is the
//! session thread itself (`own/win/host.rs`); elsewhere it refuses
//! everything, so a `--capture-worker` started off Windows still answers
//! every request, with the reason. `tests/capture_worker.rs` drives the real
//! process on Windows.
//!
//! **EOF means shutdown, never a panic.** A daemon that is killed closes the
//! worker's stdin as it goes, and the recording in flight is then worth
//! finalizing rather than abandoning: the host's `release` finalizes it and
//! the loop returns. The same lesson libobs-recorder's PR #1 learned, where
//! an `unwrap` on a closed pipe took the worker down with a file half-written.
//! (With the daemon's job object in place a killed daemon takes the worker
//! with it anyway; EOF is what covers every other way the pipe can close.)

use std::io::{BufRead, Write};
use std::path::PathBuf;

use super::protocol::{self, Line, PROTOCOL_VERSION, Reply, Request, Started};
use crate::recorder::audio::AudioLayout;
use crate::recorder::own::status::Status;

/// What the loop asks of the session. Each method is one `Command`.
pub trait Host {
    fn prepare(&mut self) -> Result<Status, String>;
    /// Brings the capture up for `path`, opening the sources `plan` names.
    /// On `Ok` the host waits for [`Host::origin`]; anything else first
    /// abandons the start.
    fn start(&mut self, path: PathBuf, plan: AudioLayout) -> Result<Started, String>;
    /// The origin for the start that was just answered.
    fn origin(&mut self, qpc_hns: i64);
    /// How the stop went, and the stop summary line if a recording was
    /// finalized (`Reply::Stopped`).
    fn stop(&mut self) -> (Result<Option<String>, String>, Option<String>);
    /// Finalize anything in flight and tear down. Returns once every capture
    /// resource is released.
    fn release(&mut self);
}

/// Why the loop ended.
#[derive(Debug, PartialEq, Eq)]
pub enum Exit {
    /// The daemon sent `Release`.
    Released,
    /// Stdin closed, or the daemon stopped reading stdout.
    Disconnected,
    /// The daemon spoke another protocol version, or never said hello.
    Refused(String),
}

impl Exit {
    /// The worker's exit code: 0 for every orderly end, which includes the
    /// daemon going away, and 2 for a handshake that failed.
    pub fn code(&self) -> i32 {
        match self {
            Exit::Released | Exit::Disconnected => 0,
            Exit::Refused(_) => 2,
        }
    }
}

/// Serves one daemon until it releases the worker or goes away. The host
/// is released on every way out, so a recording in flight is finalized
/// whichever it was.
pub fn serve(mut reader: impl BufRead, mut writer: impl Write, host: &mut impl Host) -> Exit {
    let exit = run(&mut reader, &mut writer, host);
    host.release();
    exit
}

fn run(reader: &mut impl BufRead, writer: &mut impl Write, host: &mut impl Host) -> Exit {
    // The handshake: nothing else is answered until it has happened.
    match protocol::read_line::<Request>(reader) {
        Line::Eof => return Exit::Disconnected,
        Line::Message(Request::Hello { protocol }) if protocol == PROTOCOL_VERSION => {
            let hello = Reply::Hello {
                protocol: PROTOCOL_VERSION,
                pid: std::process::id(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            };
            if protocol::write_line(writer, &hello).is_err() {
                return Exit::Disconnected;
            }
        }
        other => {
            let reason = match other {
                Line::Message(Request::Hello { protocol }) => format!(
                    "the daemon speaks protocol {protocol}, this worker {PROTOCOL_VERSION}"
                ),
                Line::Message(request) => format!("{request:?} before hello"),
                Line::Garbled(line) => format!("not a request: {line}"),
                Line::Eof => unreachable!("matched above"),
            };
            let _ = protocol::write_line(writer, &Reply::Refused { reason: reason.clone() });
            return Exit::Refused(reason);
        }
    }

    // Whether the last `Start` was answered `Ok` and its origin is due.
    let mut origin_due = false;
    loop {
        let request = match protocol::read_line::<Request>(reader) {
            Line::Eof => return Exit::Disconnected,
            Line::Garbled(line) => {
                let refused = Reply::Refused { reason: format!("not a request: {line}") };
                if protocol::write_line(writer, &refused).is_err() {
                    return Exit::Disconnected;
                }
                continue;
            }
            Line::Message(request) => request,
        };
        if origin_due && !matches!(request, Request::Origin { .. }) {
            // The start is abandoned when its origin never comes; the host
            // does that itself when it is next asked anything.
            origin_due = false;
        }
        let reply = match request {
            Request::Hello { .. } => Reply::Refused { reason: "hello twice".to_string() },
            Request::Prepare => Reply::Prepared { result: host.prepare() },
            Request::Start { path, plan } => {
                let result = host.start(path, plan);
                origin_due = result.is_ok();
                Reply::Started { result }
            }
            Request::Origin { qpc_hns } => {
                if std::mem::take(&mut origin_due) {
                    host.origin(qpc_hns);
                    continue;
                }
                Reply::Refused { reason: "an origin with no start waiting for it".to_string() }
            }
            Request::Stop => {
                let (result, summary) = host.stop();
                Reply::Stopped { result, summary }
            }
            Request::Release => return Exit::Released,
        };
        if protocol::write_line(writer, &reply).is_err() {
            return Exit::Disconnected;
        }
    }
}

/// The host off Windows: there is nothing to capture with, so every request
/// is refused with the reason, the way `FailedRecorder` refuses.
#[cfg(not(target_os = "windows"))]
pub struct Refusing;

#[cfg(not(target_os = "windows"))]
impl Host for Refusing {
    fn prepare(&mut self) -> Result<Status, String> {
        Err("the own capture backend records on Windows only".to_string())
    }

    fn start(&mut self, _path: PathBuf, _plan: AudioLayout) -> Result<Started, String> {
        Err("the own capture backend records on Windows only".to_string())
    }

    fn origin(&mut self, _qpc_hns: i64) {}

    fn stop(&mut self) -> (Result<Option<String>, String>, Option<String>) {
        (Err("not recording".to_string()), None)
    }

    fn release(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A session that records what it was asked, in order.
    #[derive(Default)]
    struct Fake {
        calls: Vec<String>,
        recording: bool,
    }

    impl Host for Fake {
        fn prepare(&mut self) -> Result<Status, String> {
            self.calls.push("prepare".into());
            Ok(Status::Ready { encoder: "fake".into() })
        }

        fn start(&mut self, path: PathBuf, plan: AudioLayout) -> Result<Started, String> {
            self.calls.push(format!("start {}", path.display()));
            if path.as_os_str() == "refuse" {
                return Err("no window".into());
            }
            self.recording = true;
            let status = Status::Ready { encoder: "fake".into() };
            Ok(Started { status, audio: plan, summary: None })
        }

        fn origin(&mut self, qpc_hns: i64) {
            self.calls.push(format!("origin {qpc_hns}"));
        }

        fn stop(&mut self) -> (Result<Option<String>, String>, Option<String>) {
            self.calls.push("stop".into());
            if std::mem::take(&mut self.recording) {
                (Ok(None), Some("own: stopped x.mp4".into()))
            } else {
                (Err("not recording".into()), None)
            }
        }

        fn release(&mut self) {
            // A recording in flight is finalized here, which is what EOF
            // has to reach.
            let finalized = if std::mem::take(&mut self.recording) { " (finalized)" } else { "" };
            self.calls.push(format!("release{finalized}"));
        }
    }

    fn lines(requests: &[Request]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for request in requests {
            protocol::write_line(&mut bytes, request).unwrap();
        }
        bytes
    }

    fn replies(bytes: &[u8]) -> Vec<Reply> {
        let mut reader = bytes;
        let mut out = Vec::new();
        loop {
            match protocol::read_line::<Reply>(&mut reader) {
                Line::Message(reply) => out.push(reply),
                Line::Eof => return out,
                Line::Garbled(line) => panic!("the worker wrote a non-message: {line}"),
            }
        }
    }

    fn no_audio() -> AudioLayout {
        AudioLayout { sources: Vec::new(), tracks: Vec::new() }
    }

    fn hello() -> Request {
        Request::Hello { protocol: PROTOCOL_VERSION }
    }

    #[test]
    fn a_whole_recording_then_release() {
        let input = lines(&[
            hello(),
            Request::Prepare,
            Request::Start { path: "x.mp4".into(), plan: no_audio() },
            Request::Origin { qpc_hns: 1234 },
            Request::Stop,
            Request::Release,
        ]);
        let mut output = Vec::new();
        let mut host = Fake::default();
        assert_eq!(serve(&input[..], &mut output, &mut host), Exit::Released);
        assert_eq!(
            host.calls,
            ["prepare", "start x.mp4", "origin 1234", "stop", "release"].map(String::from)
        );
        let replies = replies(&output);
        assert!(matches!(replies[0], Reply::Hello { protocol: PROTOCOL_VERSION, .. }));
        assert!(matches!(replies[1], Reply::Prepared { result: Ok(_) }));
        assert!(matches!(replies[2], Reply::Started { result: Ok(_) }));
        assert_eq!(
            replies[3],
            Reply::Stopped { result: Ok(None), summary: Some("own: stopped x.mp4".into()) }
        );
        assert_eq!(replies.len(), 4, "the origin and the release are not answered");
    }

    /// EOF mid-recording: the recording is finalized and the loop returns;
    /// no panic, no error exit.
    #[test]
    fn eof_mid_recording_finalizes_and_exits_cleanly() {
        let input = lines(&[
            hello(),
            Request::Start { path: "x.mp4".into(), plan: no_audio() },
            Request::Origin { qpc_hns: 1 },
        ]);
        let mut host = Fake::default();
        let exit = serve(&input[..], Vec::new(), &mut host);
        assert_eq!(exit, Exit::Disconnected);
        assert_eq!(exit.code(), 0);
        assert_eq!(host.calls.last().unwrap(), "release (finalized)");
    }

    #[test]
    fn eof_before_the_handshake_is_a_clean_exit() {
        let mut host = Fake::default();
        let exit = serve(&b""[..], Vec::new(), &mut host);
        assert_eq!(exit, Exit::Disconnected);
        assert_eq!(exit.code(), 0);
        assert_eq!(host.calls, ["release"]);
    }

    /// EOF halfway through a line: what was there is not a request, and the
    /// loop still ends cleanly.
    #[test]
    fn eof_mid_line_is_a_clean_exit() {
        let mut input = lines(&[hello()]);
        input.extend_from_slice(br#"{"type":"sto"#);
        let mut output = Vec::new();
        let mut host = Fake::default();
        assert_eq!(serve(&input[..], &mut output, &mut host), Exit::Disconnected);
        assert!(matches!(replies(&output)[1], Reply::Refused { .. }));
    }

    #[test]
    fn a_wrong_version_is_refused_and_exits_non_zero() {
        let input = lines(&[Request::Hello { protocol: PROTOCOL_VERSION + 1 }, Request::Prepare]);
        let mut output = Vec::new();
        let mut host = Fake::default();
        let exit = serve(&input[..], &mut output, &mut host);
        assert!(matches!(exit, Exit::Refused(_)));
        assert_eq!(exit.code(), 2);
        assert!(matches!(replies(&output)[..], [Reply::Refused { .. }]));
        assert_eq!(host.calls, ["release"], "nothing but the release reached the host");
    }

    #[test]
    fn a_request_before_hello_is_refused() {
        let input = lines(&[Request::Prepare]);
        let mut host = Fake::default();
        assert!(matches!(serve(&input[..], Vec::new(), &mut host), Exit::Refused(_)));
    }

    /// A garbled line is answered, and the loop carries on.
    #[test]
    fn garbage_is_refused_and_the_loop_goes_on() {
        let mut input = lines(&[hello()]);
        input.extend_from_slice(b"not json\n");
        input.extend(lines(&[Request::Prepare, Request::Release]));
        let mut output = Vec::new();
        let mut host = Fake::default();
        assert_eq!(serve(&input[..], &mut output, &mut host), Exit::Released);
        let replies = replies(&output);
        assert!(matches!(replies[1], Reply::Refused { .. }));
        assert!(matches!(replies[2], Reply::Prepared { .. }));
    }

    /// An origin is only passed on for the start that was just answered Ok.
    #[test]
    fn an_origin_without_a_start_is_refused() {
        let input = lines(&[
            hello(),
            Request::Origin { qpc_hns: 5 },
            Request::Start { path: "refuse".into(), plan: no_audio() },
            Request::Origin { qpc_hns: 6 },
            Request::Release,
        ]);
        let mut output = Vec::new();
        let mut host = Fake::default();
        serve(&input[..], &mut output, &mut host);
        assert!(!host.calls.iter().any(|c| c.starts_with("origin")), "{:?}", host.calls);
        let replies = replies(&output);
        assert!(matches!(replies[1], Reply::Refused { .. }));
        assert!(matches!(replies[2], Reply::Started { result: Err(_) }));
        assert!(matches!(replies[3], Reply::Refused { .. }));
    }

    /// The daemon stopped reading: the loop ends at the first reply it cannot
    /// write, and still releases.
    #[test]
    fn a_closed_stdout_is_a_disconnect() {
        struct Closed;
        impl Write for Closed {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let input = lines(&[hello(), Request::Prepare]);
        let mut host = Fake::default();
        assert_eq!(serve(&input[..], Closed, &mut host), Exit::Disconnected);
        assert_eq!(host.calls, ["release"]);
    }
}
