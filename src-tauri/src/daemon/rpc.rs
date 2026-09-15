//! JSON-RPC over a byte stream. WS3 task 3.1.
//!
//! The command half is `core::dispatch`, which already exists and names no
//! `tauri` type; the event half is WS2's `contract::events::Event`. This is the
//! part in between: framing, per-session state, and the fan-out that turns one
//! supervisor into many subscribers (implementation plan §4.2).
//!
//! ## Newline-delimited JSON, not a length prefix
//!
//! Every frame is one `serde_json` value on one line. A length prefix would be
//! marginally cheaper and considerably harder to debug: this way the protocol
//! can be read with `cat`, replayed with `echo`, and diffed in a test failure
//! as text. Nothing here is on a hot path; the busiest frame is a marker, at
//! roughly 1 Hz.
//!
//! JSON cannot contain a raw newline, so the delimiter cannot appear inside a
//! frame and `read_line` is a complete framer.
//!
//! ## Requests carry ids because replies can overtake each other
//!
//! A slow `extract_audio_track` must not head-of-line block a status poll, so
//! commands are answered on whatever task finishes first and the client matches
//! the reply to the request by `id` rather than by arrival order. The daemon
//! never invents an id; it echoes the one it was given.
//!
//! ## Events are a broadcast, and lag is a frame rather than a disconnect
//!
//! One `tokio::sync::broadcast` per daemon, one receiver per session. It is
//! bounded on purpose: a client that stops reading must not be able to grow the
//! daemon's memory without limit, which is exactly what an unbounded channel
//! would let a hung UI do while a game is being recorded.
//!
//! When a session falls behind, the channel drops the oldest frames and tells
//! us how many. That is reported as `Event::Lagged`, which the contract already
//! declares for this, rather than by closing the connection: the client's right
//! move is to re-`hello` for a fresh snapshot, and it cannot decide that if the
//! socket simply died.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::broadcast;

use crate::contract::events::{Event, Topic};
use crate::core::Ctx;

/// How many events the daemon buffers for a session that is not keeping up.
///
/// A 35-minute game produces a few hundred markers and samples in total, so
/// this is more than one game's worth of backlog: a session has to be wedged,
/// not merely slow, to lose anything. Past that the oldest frames go and the
/// client is told how many, which is cheaper and more honest than growing
/// without bound behind a UI that has stopped reading.
const EVENT_BUFFER: usize = 512;

/// The wire protocol's version.
///
/// Bumped when a frame changes shape in a way an older client would misread.
/// The updater can replace the daemon under a running UI, so a mismatch is a
/// real state rather than a theoretical one: `hello` refuses it and says so,
/// and the UI's side of that refusal is WS3.4.
pub const PROTOCOL: u32 = 1;

/// What a client sends.
#[derive(Debug, Deserialize)]
#[serde(tag = "method", rename_all = "camelCase")]
pub enum Request {
    /// The handshake. Must be first, and carries the client's protocol version
    /// so a skewed pair refuses rather than misreading each other's frames.
    Hello { id: u64, protocol: u32 },
    /// Which topics this session wants. Replacing, not adding: a client states
    /// what it wants now, so re-subscribing is how it narrows as well as widens
    /// and there is no `unsubscribe` to keep in step.
    Subscribe { id: u64, topics: Vec<Topic> },
    /// Run a command from `core::dispatch`'s table.
    Invoke {
        id: u64,
        command: String,
        #[serde(default)]
        args: Value,
    },
}

/// What the daemon sends.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Reply {
    /// The handshake's answer.
    #[serde(rename_all = "camelCase")]
    Hello { id: u64, protocol: u32 },
    /// A command succeeded. `value` is whatever the command returns, already
    /// serialized by the dispatcher.
    Ok { id: u64, value: Value },
    /// A command failed, or the frame could not be understood. A business
    /// error and a protocol error look the same here on purpose: both are
    /// something the caller has to show a person, and neither is fatal to the
    /// session.
    Err { id: u64, error: String },
    /// An event. Carries no id, because nothing asked for it.
    Event { event: Event },
}

/// A frame with no id to echo, because the id is what could not be read.
const NO_ID: u64 = 0;

/// The daemon's event fan-out.
///
/// Cloneable, so `lib.rs` can hand one clone to `Supervisor::set_event_sink`
/// and keep another to make receivers from.
#[derive(Clone)]
pub struct Events(broadcast::Sender<Event>);

impl Events {
    pub fn new() -> Self {
        Self(broadcast::channel(EVENT_BUFFER).0)
    }

    /// Publishes to every session. Deliberately infallible: `broadcast::send`
    /// errors only when there are no receivers, which is the ordinary state of
    /// a daemon with no UI attached and not something a recording should care
    /// about.
    pub fn publish(&self, event: Event) {
        let _ = self.0.send(event);
    }

    fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.0.subscribe()
    }
}

impl Default for Events {
    fn default() -> Self {
        Self::new()
    }
}

/// What one connection remembers.
struct Session {
    /// Empty until `subscribe`. A client that never subscribes gets replies and
    /// no events, which is a legitimate thing to want: `spawn.rs`'s liveness
    /// probe does exactly that.
    topics: Vec<Topic>,
    greeted: bool,
}

/// Serves one connection until it closes or the protocol is violated.
///
/// Generic over the stream so the same code serves a Windows named pipe in
/// production and a Unix socket in the tests. That is not a convenience: the
/// alternative is a protocol whose only exercise is on the Windows box, which
/// is the loop this project is organised to stay out of.
pub async fn serve<S>(stream: S, ctx: Arc<Ctx>, events: Events) -> std::io::Result<()>
where
    S: tokio::io::AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (rx, tx) = tokio::io::split(stream);
    let mut lines = BufReader::new(rx).lines();
    let writer = Arc::new(tokio::sync::Mutex::new(tx));

    // Subscribed from the start, not from `subscribe`: a client that asks for
    // topics should not miss what happened between its `hello` and its
    // subscription. Frames for topics it has not asked for are dropped when
    // they are written, which costs nothing and closes that window.
    let mut rx_events = events.subscribe();
    let session = Arc::new(tokio::sync::Mutex::new(Session { topics: Vec::new(), greeted: false }));

    let pump = {
        let writer = Arc::clone(&writer);
        let session = Arc::clone(&session);
        tokio::spawn(async move {
            loop {
                match rx_events.recv().await {
                    Ok(event) => {
                        let wanted = {
                            let s = session.lock().await;
                            s.greeted && s.topics.contains(&event.topic())
                        };
                        if wanted && write_frame(&writer, &Reply::Event { event }).await.is_err() {
                            return;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(dropped)) => {
                        // The contract declares this for exactly this moment.
                        let event = Event::Lagged { dropped: dropped as u32 };
                        if write_frame(&writer, &Reply::Event { event }).await.is_err() {
                            return;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => return,
                }
            }
        })
    };

    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Request>(&line) {
            Ok(request) => handle(request, &ctx, &session).await,
            // A frame we cannot parse has no id to echo, so the error goes out
            // under `NO_ID`. Answering at all is deliberate: silence would look
            // identical to a hung daemon from the other end.
            Err(e) => Reply::Err { id: NO_ID, error: format!("malformed frame: {e}") },
        };
        if write_frame(&writer, &reply).await.is_err() {
            break;
        }
    }

    pump.abort();
    Ok(())
}

async fn handle(
    request: Request,
    ctx: &Arc<Ctx>,
    session: &Arc<tokio::sync::Mutex<Session>>,
) -> Reply {
    match request {
        Request::Hello { id, protocol } => {
            if protocol != PROTOCOL {
                // Refuse rather than adapt. The updater can replace the daemon
                // under a running UI, and a daemon that tried to speak an older
                // dialect would be guessing at frames it has never seen.
                return Reply::Err {
                    id,
                    error: format!(
                        "protocol {protocol} is not {PROTOCOL}; this client and daemon are from \
                         different builds"
                    ),
                };
            }
            session.lock().await.greeted = true;
            Reply::Hello { id, protocol: PROTOCOL }
        }
        Request::Subscribe { id, topics } => {
            let mut s = session.lock().await;
            if !s.greeted {
                return Reply::Err { id, error: "subscribe before hello".to_string() };
            }
            s.topics = topics;
            Reply::Ok { id, value: Value::Null }
        }
        Request::Invoke { id, command, args } => {
            {
                let s = session.lock().await;
                if !s.greeted {
                    return Reply::Err { id, error: "invoke before hello".to_string() };
                }
            }
            match invoke(ctx, &command, args).await {
                Ok(value) => Reply::Ok { id, value },
                Err(error) => Reply::Err { id, error },
            }
        }
    }
}

/// Runs one command on the right kind of thread.
///
/// Only `lcu_status` is genuinely async; everything else is blocking work
/// against SQLite or the filesystem. Running those on the runtime's worker
/// threads would stall every other session behind the slowest of them, which is
/// what `spawn_blocking` exists to avoid and what the per-request ids would
/// otherwise be unable to deliver on.
async fn invoke(ctx: &Arc<Ctx>, command: &str, args: Value) -> Result<Value, String> {
    if crate::core::is_async_command(command) {
        crate::core::dispatch(ctx, command, args).await
    } else {
        let ctx = Arc::clone(ctx);
        let command = command.to_string();
        tokio::task::spawn_blocking(move || crate::core::dispatch_blocking(&ctx, &command, args))
            .await
            .map_err(|e| format!("command task failed: {e}"))?
    }
}

/// One frame, one line.
///
/// The lock is held across the write so two tasks cannot interleave halves of
/// two frames into one line, which would be a protocol error the reader could
/// not recover from.
async fn write_frame<W>(writer: &Arc<tokio::sync::Mutex<W>>, reply: &Reply) -> std::io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    let mut line = serde_json::to_string(reply).map_err(std::io::Error::other)?;
    line.push('\n');
    let mut w = writer.lock().await;
    w.write_all(line.as_bytes()).await?;
    w.flush().await
}

#[cfg(test)]
mod tests {
    //! Over a Unix socket, which is the point.
    //!
    //! The production transport is a Windows named pipe, and a protocol whose
    //! only exercise is on the Windows box is one that gets tested once a week.
    //! `serve` is generic over the stream precisely so the same code can be
    //! driven here in milliseconds.

    use super::*;
    use crate::core::Ctx;
    use crate::db::Db;
    use crate::recorder::Recorder;
    use crate::recorder::stub::StubRecorder;
    use crate::state_machine;
    use std::sync::Mutex;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::{UnixListener, UnixStream};

    fn ctx() -> Arc<Ctx> {
        let recorder: Arc<Mutex<Box<dyn Recorder>>> =
            Arc::new(Mutex::new(Box::new(StubRecorder::new())));
        let db = Arc::new(Db::open_temporary().unwrap());
        let dir = std::env::temp_dir().join(format!("nr-rpc-test-{}", std::process::id()));
        let supervisor =
            state_machine::Supervisor::new(Arc::clone(&recorder), dir.clone(), Arc::clone(&db));
        Arc::new(Ctx::new(recorder, supervisor, db, dir.clone(), dir.join("ddragon"), None))
    }

    /// Spins up a server on a throwaway socket and returns a connected client.
    async fn connected(events: Events) -> (BufReader<tokio::net::unix::OwnedReadHalf>, tokio::net::unix::OwnedWriteHalf) {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "nr-rpc-{}-{}.sock",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();

        let ctx = ctx();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let _ = serve(stream, ctx, events).await;
        });

        let client = UnixStream::connect(&path).await.unwrap();
        let _ = std::fs::remove_file(&path);
        let (rx, tx) = client.into_split();
        (BufReader::new(rx), tx)
    }

    async fn send(tx: &mut tokio::net::unix::OwnedWriteHalf, frame: &str) {
        tx.write_all(frame.as_bytes()).await.unwrap();
        tx.write_all(b"\n").await.unwrap();
        tx.flush().await.unwrap();
    }

    async fn next(rx: &mut BufReader<tokio::net::unix::OwnedReadHalf>) -> Value {
        let mut line = String::new();
        rx.read_line(&mut line).await.unwrap();
        serde_json::from_str(&line).unwrap()
    }

    #[tokio::test]
    async fn hello_is_answered_with_the_protocol_version() {
        let (mut rx, mut tx) = connected(Events::new()).await;
        send(&mut tx, r#"{"method":"hello","id":1,"protocol":1}"#).await;
        let reply = next(&mut rx).await;
        assert_eq!(reply["type"], "hello");
        assert_eq!(reply["id"], 1);
        assert_eq!(reply["protocol"], PROTOCOL);
    }

    /// The updater can replace the daemon under a running UI, so this is a real
    /// state rather than a theoretical one.
    #[tokio::test]
    async fn a_skewed_protocol_is_refused_and_says_why() {
        let (mut rx, mut tx) = connected(Events::new()).await;
        send(&mut tx, r#"{"method":"hello","id":7,"protocol":999}"#).await;
        let reply = next(&mut rx).await;
        assert_eq!(reply["type"], "err");
        assert_eq!(reply["id"], 7);
        let msg = reply["error"].as_str().unwrap();
        assert!(msg.contains("999") && msg.contains("different builds"), "{msg}");
    }

    #[tokio::test]
    async fn a_command_before_hello_is_refused() {
        let (mut rx, mut tx) = connected(Events::new()).await;
        send(&mut tx, r#"{"method":"invoke","id":1,"command":"list_recordings"}"#).await;
        let reply = next(&mut rx).await;
        assert_eq!(reply["type"], "err");
        assert!(reply["error"].as_str().unwrap().contains("before hello"));
    }

    /// Answering at all matters: silence looks identical to a hung daemon.
    #[tokio::test]
    async fn a_malformed_frame_is_answered_rather_than_ignored() {
        let (mut rx, mut tx) = connected(Events::new()).await;
        send(&mut tx, "{not json").await;
        let reply = next(&mut rx).await;
        assert_eq!(reply["type"], "err");
        assert!(reply["error"].as_str().unwrap().contains("malformed frame"));
        // And the session survives it.
        send(&mut tx, r#"{"method":"hello","id":2,"protocol":1}"#).await;
        assert_eq!(next(&mut rx).await["type"], "hello");
    }

    /// **The exit criterion.** 100 interleaved requests, every one answered,
    /// matched by id rather than by arrival order.
    ///
    /// They are sent without waiting for replies on purpose: that is what makes
    /// them interleaved, and it is the case per-request ids exist for. A slow
    /// command must not head-of-line block a fast one, so the replies are
    /// allowed to come back in any order and only the set has to be right.
    #[tokio::test]
    async fn a_hundred_interleaved_requests_are_all_answered() {
        let (mut rx, mut tx) = connected(Events::new()).await;
        send(&mut tx, r#"{"method":"hello","id":0,"protocol":1}"#).await;
        assert_eq!(next(&mut rx).await["type"], "hello");

        // A mix of read, write and unknown, so the blocking path, the async
        // path and the error path are all in the interleaving.
        let commands = ["list_recordings", "get_disk_usage", "get_ui_prefs", "no_such_command"];
        for id in 1..=100u64 {
            let command = commands[(id as usize) % commands.len()];
            send(
                &mut tx,
                &format!(r#"{{"method":"invoke","id":{id},"command":"{command}","args":{{}}}}"#),
            )
            .await;
        }

        let mut seen = std::collections::BTreeSet::new();
        while seen.len() < 100 {
            let reply = next(&mut rx).await;
            // Events would have no id; none are published in this test.
            let id = reply["id"].as_u64().expect("every reply echoes its request id");
            assert!(seen.insert(id), "id {id} was answered twice");
            let kind = reply["type"].as_str().unwrap();
            assert!(kind == "ok" || kind == "err", "unexpected frame {reply}");
        }
        assert_eq!(seen.first(), Some(&1));
        assert_eq!(seen.last(), Some(&100));
    }

    /// **The other half of the exit criterion:** events delivered, and in the
    /// order they were published.
    #[tokio::test]
    async fn events_arrive_in_order_on_a_subscribed_topic() {
        let events = Events::new();
        let (mut rx, mut tx) = connected(events.clone()).await;
        send(&mut tx, r#"{"method":"hello","id":1,"protocol":1}"#).await;
        assert_eq!(next(&mut rx).await["type"], "hello");
        send(&mut tx, r#"{"method":"subscribe","id":2,"topics":["library"]}"#).await;
        assert_eq!(next(&mut rx).await["type"], "ok");

        for i in 0..20u64 {
            events.publish(Event::RetentionRan { deleted: vec![i as i64], freed_bytes: i as i64 });
        }

        for i in 0..20u64 {
            let frame = next(&mut rx).await;
            assert_eq!(frame["type"], "event");
            assert_eq!(
                frame["event"]["deleted"][0].as_u64(),
                Some(i),
                "events must arrive in publication order"
            );
        }
    }

    /// A client only hears what it asked for.
    #[tokio::test]
    async fn an_unsubscribed_topic_is_not_delivered() {
        let events = Events::new();
        let (mut rx, mut tx) = connected(events.clone()).await;
        send(&mut tx, r#"{"method":"hello","id":1,"protocol":1}"#).await;
        assert_eq!(next(&mut rx).await["type"], "hello");
        send(&mut tx, r#"{"method":"subscribe","id":2,"topics":["library"]}"#).await;
        assert_eq!(next(&mut rx).await["type"], "ok");

        // `lcu` is not subscribed; `library` is. Only the second should arrive.
        events.publish(Event::LcuPhase { phase: None, client_present: false });
        events.publish(Event::RetentionRan { deleted: vec![9], freed_bytes: 1 });

        let frame = next(&mut rx).await;
        assert_eq!(frame["event"]["type"], "retentionRan");
    }

    /// Lag is a frame, not a disconnect: the client's right move is to
    /// re-`hello` for a fresh snapshot, and it cannot decide that if the socket
    /// simply died.
    #[tokio::test]
    async fn a_session_that_falls_behind_is_told_how_much_it_missed() {
        let events = Events::new();
        let (mut rx, mut tx) = connected(events.clone()).await;
        send(&mut tx, r#"{"method":"hello","id":1,"protocol":1}"#).await;
        assert_eq!(next(&mut rx).await["type"], "hello");
        send(&mut tx, r#"{"method":"subscribe","id":2,"topics":["library"]}"#).await;
        assert_eq!(next(&mut rx).await["type"], "ok");

        // Comfortably past the buffer, published faster than the pump drains.
        for i in 0..(EVENT_BUFFER as i64 * 3) {
            events.publish(Event::RetentionRan { deleted: vec![i], freed_bytes: i });
        }

        let mut saw_lagged = false;
        for _ in 0..64 {
            let frame = next(&mut rx).await;
            if frame["event"]["type"] == "lagged" {
                assert!(frame["event"]["dropped"].as_u64().unwrap() > 0);
                saw_lagged = true;
                break;
            }
        }
        assert!(saw_lagged, "a session that overflowed the buffer must be told");
    }
}
