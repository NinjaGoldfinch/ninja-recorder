//! The real binary in `--capture-worker` mode, as a process (#241).
//!
//! The protocol, the loop and its EOF handling are unit-tested in memory
//! (`recorder::own::worker`). What only a process can show is `main.rs`
//! dispatching the flag before anything else is built, the worker taking
//! stdout for itself so nothing else lands on the channel, and the exit code
//! at each way out. The worker behind the handshake is the real one, with
//! the session thread and Media Foundation.
//!
//! **Windows only.** Off Windows the binary links the GTK stack Tauri needs
//! there, so it will not even load on a box without those runtime libraries,
//! and CI's `test` job runs on `windows-latest` alone. The worker's loop is
//! the same code on every host and is tested in memory there.
//!
//! Spoken as raw JSON on purpose, rather than through the crate's types: it is
//! the wire a daemon and a worker from the same build agree on, and this test
//! links nothing of the library, so it cannot pull the GUI stack into a test
//! binary that has no application manifest (DEVELOPMENT.md §12).

#![cfg(target_os = "windows")]

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

const EXE: &str = env!("CARGO_BIN_EXE_ninja-recorder");

fn spawn() -> (Child, BufReader<ChildStdout>) {
    let logs = std::env::temp_dir().join(format!("ninja-capture-worker-test-{}", std::process::id()));
    let mut child = Command::new(EXE)
        .arg("--capture-worker")
        // Its log goes here rather than into the user's data folder.
        .env("NINJA_CAPTURE_WORKER_LOG_DIR", &logs)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn the worker");
    let stdout = BufReader::new(child.stdout.take().unwrap());
    (child, stdout)
}

fn send(child: &mut Child, line: &str) {
    let stdin = child.stdin.as_mut().expect("stdin open");
    writeln!(stdin, "{line}").expect("write a request");
    stdin.flush().unwrap();
}

/// One reply. Anything on stdout that is not a JSON object is a failure:
/// the channel must carry messages and nothing else.
fn reply(stdout: &mut BufReader<ChildStdout>) -> serde_json::Value {
    let mut line = String::new();
    let n = stdout.read_line(&mut line).expect("read a reply");
    assert!(n > 0, "the worker closed stdout instead of answering");
    let value: serde_json::Value = serde_json::from_str(line.trim_end())
        .unwrap_or_else(|e| panic!("stdout carried a non-message ({e}): {line:?}"));
    assert!(value.is_object(), "{line:?}");
    value
}

fn handshake(child: &mut Child, stdout: &mut BufReader<ChildStdout>) {
    send(child, r#"{"type":"hello","protocol":1}"#);
    let hello = reply(stdout);
    assert_eq!(hello["type"], "hello", "{hello}");
    assert_eq!(hello["protocol"], 1, "{hello}");
    assert_eq!(hello["pid"], child.id(), "{hello}");
}

/// Waits for the worker to exit, killing it (and failing) if it does not.
fn exit_code(child: &mut Child) -> i32 {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if let Some(status) = child.try_wait().expect("try_wait") {
            return status.code().unwrap_or_else(|| panic!("no exit code: {status}"));
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("the worker did not exit");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Handshake, a prepare (which on Windows brings Media Foundation up in the
/// worker, logging as it goes, and must still leave the channel clean),
/// `Release`, and a clean exit with nothing after the last reply.
#[test]
fn the_worker_shakes_hands_prepares_and_exits_cleanly_on_release() {
    let (mut child, mut stdout) = spawn();
    handshake(&mut child, &mut stdout);

    send(&mut child, r#"{"type":"prepare"}"#);
    let prepared = reply(&mut stdout);
    assert_eq!(prepared["type"], "prepared", "{prepared}");
    // `Ok` on a runner with Media Foundation and an adapter, `Err` with the
    // reason on one without. Either is an
    // answer; what is under test is that it came back as one line.
    let result = &prepared["result"];
    assert!(result.get("Ok").is_some() || result.get("Err").is_some(), "{prepared}");
    eprintln!("[capture worker test] prepare answered {result}");

    send(&mut child, r#"{"type":"stop"}"#);
    let stopped = reply(&mut stdout);
    assert_eq!(stopped["type"], "stopped", "{stopped}");
    assert!(stopped["result"].get("Err").is_some(), "nothing was recording: {stopped}");

    send(&mut child, r#"{"type":"release"}"#);
    assert_eq!(exit_code(&mut child), 0);
    let mut rest = String::new();
    stdout.read_line(&mut rest).unwrap();
    assert_eq!(rest, "", "nothing after the last reply");
}

/// The daemon going away: stdin closes. That is a shutdown, not a crash.
#[test]
fn the_worker_exits_cleanly_when_stdin_closes() {
    let (mut child, mut stdout) = spawn();
    handshake(&mut child, &mut stdout);
    drop(child.stdin.take());
    assert_eq!(exit_code(&mut child), 0);
}

/// Stdin closing before a word was said, too.
#[test]
fn the_worker_exits_cleanly_when_stdin_closes_before_the_handshake() {
    let (mut child, _stdout) = spawn();
    drop(child.stdin.take());
    assert_eq!(exit_code(&mut child), 0);
}

/// A daemon from another protocol version is refused, and the worker leaves
/// with a code that says so rather than serving it.
#[test]
fn the_worker_refuses_another_protocol() {
    let (mut child, mut stdout) = spawn();
    send(&mut child, r#"{"type":"hello","protocol":999}"#);
    let refused = reply(&mut stdout);
    assert_eq!(refused["type"], "refused", "{refused}");
    assert_eq!(exit_code(&mut child), 2);
}
