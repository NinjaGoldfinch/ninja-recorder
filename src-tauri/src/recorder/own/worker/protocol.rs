//! What the daemon and the capture worker say to each other, and how it is
//! framed. Both sides use this module and nothing else to speak the protocol.
//!
//! **One JSON value per line**, daemon to worker on the worker's stdin and
//! worker to daemon on its stdout. `serde_json` escapes every newline inside a
//! string, so a line is always exactly one message. Nothing else may be
//! written to the worker's stdout: the worker takes the handle for itself and
//! points the process's standard output somewhere harmless before anything
//! else runs (`worker::run`), which is the bug #221 found in the libobs worker,
//! where the library's own logging shared the channel.
//!
//! The messages are the session thread's `Command`s (`own/win/session.rs`)
//! and their answers, one for one. `Start` is the one that takes two lines
//! from the daemon: the path, and then, once the worker has said the capture
//! is up, the `Origin`, which is the QPC instant the file's t = 0 is. QPC is
//! one clock for the whole machine, so the daemon reads it and the worker's
//! session uses it unchanged.
//!
//! **Not the UI contract.** Nothing here crosses to the frontend, so none of it
//! derives `ts_rs::TS` and none of it is in `contract/`. Both ends are always
//! the same binary; the `Hello` exchange is there to catch the day they are
//! not (a daemon from before an update spawning a replaced executable).

use std::io::{self, BufRead, Write};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::recorder::own::status::Status;

/// Bumped on any change to the messages below. The worker refuses a daemon
/// that says another number, and the daemon refuses a worker that answers
/// with one.
pub const PROTOCOL_VERSION: u32 = 1;

/// Daemon to worker.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    /// The first line, always. Answered with [`Reply::Hello`].
    Hello { protocol: u32 },
    /// The pre-warm. Answered with [`Reply::Prepared`].
    Prepare,
    /// Start recording to `path`. Answered with [`Reply::Started`], and on
    /// success followed by exactly one [`Request::Origin`].
    Start { path: PathBuf },
    /// The file's t = 0, in QPC 100 ns units, read by the daemon as the last
    /// thing its `start` does. No answer.
    Origin { qpc_hns: i64 },
    /// Stop and finalize. Answered with [`Reply::Stopped`].
    Stop,
    /// Finalize anything in flight, tear down and exit. No answer: the
    /// worker's exit is the answer.
    Release,
}

/// Worker to daemon.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Reply {
    /// The worker's side of the handshake.
    Hello { protocol: u32, pid: u32, version: String },
    /// The ranked status, or why nothing could be brought up.
    Prepared { result: Result<Status, String> },
    /// The capture is up and waiting for its origin, or why it is not.
    Started { result: Result<Started, String> },
    /// `Ok(None)` for a clean stop, `Ok(Some(_))` for a recording that ended
    /// early but was finalized, `Err` when the finalize itself failed.
    Stopped { result: Result<Option<String>, String> },
    /// A line the worker could not act on: not JSON, the wrong version, or a
    /// message out of order. The daemon treats it as a broken worker.
    Refused { reason: String },
}

/// What a start answers with: the session's `Started`, as plain data.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Started {
    /// The status for the encoder that actually loaded.
    pub status: Status,
    /// Whether the file has the game's audio track.
    pub game_audio: bool,
}

/// Writes one message and flushes it: the other side is waiting on the line.
pub fn write_line<T: Serialize>(writer: &mut impl Write, message: &T) -> io::Result<()> {
    let line = serde_json::to_string(message).map_err(io::Error::other)?;
    writer.write_all(line.as_bytes())?;
    writer.write_all(b"\n")?;
    writer.flush()
}

/// What reading one line produced.
#[derive(Debug)]
pub enum Line<T> {
    Message(T),
    /// The line was not a message of this type; kept for the refusal.
    Garbled(String),
    /// The other end closed its side.
    Eof,
}

/// Reads one message. A read error is reported as `Eof`: on a pipe it means
/// the same thing, that the other end has gone.
pub fn read_line<T: for<'de> Deserialize<'de>>(reader: &mut impl BufRead) -> Line<T> {
    let mut line = String::new();
    match reader.read_line(&mut line) {
        Ok(0) | Err(_) => Line::Eof,
        Ok(_) => parse(&line),
    }
}

/// One line, already read, as a message.
pub fn parse<T: for<'de> Deserialize<'de>>(line: &str) -> Line<T> {
    match serde_json::from_str(line.trim_end()) {
        Ok(message) => Line::Message(message),
        Err(e) => Line::Garbled(format!("{e}: {}", line.trim_end())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip<T>(message: T)
    where
        T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug,
    {
        let mut bytes = Vec::new();
        write_line(&mut bytes, &message).unwrap();
        assert_eq!(bytes.iter().filter(|&&b| b == b'\n').count(), 1, "one line per message");
        assert_eq!(*bytes.last().unwrap(), b'\n');
        match read_line::<T>(&mut &bytes[..]) {
            Line::Message(back) => assert_eq!(back, message),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn every_request_round_trips() {
        for request in [
            Request::Hello { protocol: PROTOCOL_VERSION },
            Request::Prepare,
            Request::Start { path: PathBuf::from(r"C:\Users\a b\recordings\x.mp4") },
            Request::Origin { qpc_hns: i64::MAX - 1 },
            Request::Origin { qpc_hns: -5 },
            Request::Stop,
            Request::Release,
        ] {
            round_trip(request);
        }
    }

    #[test]
    fn every_reply_round_trips() {
        let software =
            Status::Software { encoder: "MS".into(), reason: "no hardware\nencoder".into() };
        for reply in [
            Reply::Hello { protocol: 1, pid: 42, version: "0.8.0".into() },
            Reply::Prepared { result: Ok(Status::Ready { encoder: "NVENC [VEN_10DE]".into() }) },
            Reply::Prepared { result: Ok(software.clone()) },
            Reply::Prepared { result: Ok(Status::Idle) },
            Reply::Prepared { result: Ok(Status::Unavailable { reason: "x".into() }) },
            Reply::Prepared { result: Err("MFStartup failed".into()) },
            Reply::Started { result: Ok(Started { status: software, game_audio: true }) },
            Reply::Started { result: Err("no game window".into()) },
            Reply::Stopped { result: Ok(None) },
            Reply::Stopped { result: Ok(Some("the game window closed".into())) },
            Reply::Stopped { result: Err("finalize failed".into()) },
            Reply::Refused { reason: "what".into() },
        ] {
            round_trip(reply);
        }
    }

    /// A message carrying a newline in a string is still one line, or the
    /// framing would split it in two.
    #[test]
    fn a_newline_inside_a_message_does_not_end_the_line() {
        let reply = Reply::Refused { reason: "one\ntwo\r\nthree".into() };
        let mut bytes = Vec::new();
        write_line(&mut bytes, &reply).unwrap();
        assert_eq!(bytes.iter().filter(|&&b| b == b'\n').count(), 1);
    }

    /// The wire shape, pinned: the Windows CI test and anything reading a
    /// capture by hand speak it as text.
    #[test]
    fn the_wire_shape_is_tagged_snake_case() {
        let hello = serde_json::to_string(&Request::Hello { protocol: 1 }).unwrap();
        assert_eq!(hello, r#"{"type":"hello","protocol":1}"#);
        assert_eq!(serde_json::to_string(&Request::Release).unwrap(), r#"{"type":"release"}"#);
        let stopped = serde_json::to_string(&Reply::Stopped { result: Ok(None) }).unwrap();
        assert_eq!(stopped, r#"{"type":"stopped","result":{"Ok":null}}"#);
    }

    #[test]
    fn a_closed_stream_is_eof_and_garbage_is_garbled() {
        assert!(matches!(read_line::<Request>(&mut &b""[..]), Line::Eof));
        assert!(matches!(read_line::<Request>(&mut &b"not json\n"[..]), Line::Garbled(_)));
        assert!(matches!(
            read_line::<Request>(&mut &br#"{"type":"launch_missiles"}"#[..]),
            Line::Garbled(_)
        ));
        // A last line with no newline is still a message.
        assert!(matches!(
            read_line::<Request>(&mut &br#"{"type":"stop"}"#[..]),
            Line::Message(Request::Stop)
        ));
    }
}
