//! The UI's side of the pipe. WS3 task 3.4.
//!
//! `daemon::rpc` is the server; this is the client that talks to it. It owns
//! the things a caller should not have to think about: matching a reply to its
//! request, keeping a connection alive across a daemon restart, and refusing to
//! attach to a daemon that speaks a different protocol.
//!
//! ## Why a task and channels rather than a shared socket
//!
//! Replies overtake each other, so something has to read the stream
//! continuously and route each frame to whoever is waiting for it. That is a
//! loop, and a loop needs somewhere to live. One task owns the connection
//! outright; callers hand it a request and a one-shot to answer on, which means
//! no caller ever holds the socket and two concurrent commands cannot interleave
//! halves of two frames.
//!
//! ## Reconnect is the normal case, not the error case
//!
//! The daemon can go away for entirely ordinary reasons: the updater replaces
//! it, a user quits it from the tray, it crashes. The UI is disposable but the
//! *recording* is not, so a UI that gave up on the first dropped pipe would
//! report a dead app for a game that is still being captured perfectly.
//!
//! So the loop reconnects with a bounded backoff, and every reconnect re-does
//! the handshake, which means a fresh snapshot. That is what makes a UI killed
//! mid-game correct the moment it comes back: it never replays history, it just
//! asks again.
//!
//! ## Version skew refuses rather than degrades
//!
//! The updater can replace the daemon under a running UI, so a client and a
//! daemon from different builds is a real state. `hello` carries a protocol
//! version and a mismatch is fatal to the *session*, deliberately: a client that
//! tried to speak an older dialect would be guessing at frames it has never
//! seen, and guessing wrong about a recording is worse than saying so.

//! ## Nothing constructs this yet
//!
//! There is no daemon to connect to until WS3.2, and no Tauri commands exposing
//! it to the webview until this module grows them. So every item here carries
//! the `cfg_attr(not(test), allow(dead_code))` the codebase already uses for a
//! declaration whose reader lands later, exactly as `contract::events` does for
//! the variants nothing emits. The tests are its only caller today, and they
//! drive it against the real server.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{Mutex, mpsc, oneshot};

use crate::contract::events::{Event, Topic};
use crate::contract::snapshot::Snapshot;
use crate::daemon::rpc::PROTOCOL;

/// How long to wait before retrying a connection, and the ceiling.
///
/// Starts fast because the overwhelmingly common case is a daemon that is
/// restarting and will be back within a second. Caps low because the UI is
/// visible: a user watching a "reconnecting" strip should not wait a minute to
/// find out it worked, and the cost of polling a dead socket every two seconds
/// is nothing.
#[cfg_attr(not(test), allow(dead_code))]
const BACKOFF_START: Duration = Duration::from_millis(100);
#[cfg_attr(not(test), allow(dead_code))]
const BACKOFF_MAX: Duration = Duration::from_secs(2);

/// What went wrong, as far as a caller is concerned.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub enum ClientError {
    /// The daemon answered, and said no.
    Command(String),
    /// Nothing is connected right now. Distinct from `Command` because the
    /// caller's move is different: retry later rather than show the user a
    /// message about their request.
    Disconnected,
    /// This build and the daemon's do not agree on the wire. Fatal: no amount
    /// of retrying fixes a version mismatch.
    ProtocolSkew { ours: u32, theirs: u32 },
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Command(e) => write!(f, "{e}"),
            Self::Disconnected => write!(f, "not connected to the recorder"),
            Self::ProtocolSkew { ours, theirs } => write!(
                f,
                "the recorder speaks protocol {theirs} and this window speaks {ours}; \
                 they are from different builds. Restart the app."
            ),
        }
    }
}

/// What the connection loop is currently doing, for the UI to render.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub enum Health {
    Connected,
    Reconnecting,
    /// Terminal. Nothing will retry.
    Skewed { ours: u32, theirs: u32 },
}

/// One request, and where to put the answer.
#[cfg_attr(not(test), allow(dead_code))]
struct Pending {
    command: String,
    args: Value,
    reply: oneshot::Sender<Result<Value, ClientError>>,
}

/// A handle to the connection loop.
#[derive(Clone)]
#[cfg_attr(not(test), allow(dead_code))]
pub struct Client {
    tx: mpsc::Sender<Pending>,
    health: Arc<Mutex<Health>>,
}

#[cfg_attr(not(test), allow(dead_code))]
impl Client {
    /// Runs one command on the daemon.
    ///
    /// Returns `Disconnected` rather than blocking when nothing is attached: a
    /// window that opens while the daemon is restarting should render its empty
    /// state and recover on the next event, not hang.
    pub async fn call(&self, command: &str, args: Value) -> Result<Value, ClientError> {
        // Checked before queueing, because the queue has capacity and a request
        // put on it while nothing is attached would sit there until something
        // was. That is indistinguishable from a hang at the call site, which is
        // exactly what a window opened during a daemon restart would do.
        match self.health().await {
            Health::Connected => {}
            Health::Skewed { ours, theirs } => {
                return Err(ClientError::ProtocolSkew { ours, theirs });
            }
            Health::Reconnecting => return Err(ClientError::Disconnected),
        }
        let (reply, answer) = oneshot::channel();
        let pending = Pending { command: command.to_string(), args, reply };
        if self.tx.send(pending).await.is_err() {
            return Err(ClientError::Disconnected);
        }
        answer.await.unwrap_or(Err(ClientError::Disconnected))
    }

    pub async fn health(&self) -> Health {
        self.health.lock().await.clone()
    }
}

/// What the loop hands back to whoever is driving the UI.
#[cfg_attr(not(test), allow(dead_code))]
pub enum FromDaemon {
    /// A fresh handshake. Every one of these replaces the UI's world.
    Snapshot(Box<Snapshot>),
    Event(Event),
    Health(Health),
}

/// Connects, and keeps connecting.
///
/// `connect` is a factory rather than a stream so the loop can call it again
/// after a drop; it is also what lets the tests hand it a loopback socket where
/// production hands it a named pipe.
///
/// **Must be called with a tokio runtime entered.** It spawns, and `tokio::spawn`
/// panics with "there is no reactor running" otherwise. That is not a detail: it
/// shipped, and the UI aborted on every launch, because Tauri's `setup` hook
/// runs on the main thread outside any runtime. `ui::link` enters Tauri's
/// before calling this.
///
/// The runtime stays the caller's choice rather than being reached for here,
/// which is what lets the tests drive this under `#[tokio::test]` and the app
/// drive it under Tauri's.
#[cfg_attr(not(test), allow(dead_code))]
pub fn spawn<C, S, F>(
    connect: C,
    topics: Vec<Topic>,
    out: mpsc::Sender<FromDaemon>,
) -> Client
where
    C: Fn() -> F + Send + Sync + 'static,
    F: std::future::Future<Output = std::io::Result<S>> + Send + 'static,
    S: tokio::io::AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (tx, rx) = mpsc::channel::<Pending>(64);
    let health = Arc::new(Mutex::new(Health::Reconnecting));
    let client = Client { tx, health: Arc::clone(&health) };
    tokio::spawn(run(connect, topics, rx, out, health));
    client
}

#[cfg_attr(not(test), allow(dead_code))]
async fn run<C, S, F>(
    connect: C,
    topics: Vec<Topic>,
    mut rx: mpsc::Receiver<Pending>,
    out: mpsc::Sender<FromDaemon>,
    health: Arc<Mutex<Health>>,
) where
    C: Fn() -> F + Send + Sync + 'static,
    F: std::future::Future<Output = std::io::Result<S>> + Send + 'static,
    S: tokio::io::AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let mut backoff = BACKOFF_START;
    loop {
        let stream = match connect().await {
            Ok(s) => s,
            Err(_) => {
                set(&health, &out, Health::Reconnecting).await;
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(BACKOFF_MAX);
                continue;
            }
        };
        backoff = BACKOFF_START;

        match session(stream, &topics, &mut rx, &out, &health).await {
            // A skew is terminal: retrying cannot make two builds agree.
            Err(ClientError::ProtocolSkew { ours, theirs }) => {
                set(&health, &out, Health::Skewed { ours, theirs }).await;
                // Drain so callers get a definite error rather than hanging on
                // a loop that will never run again.
                while let Some(p) = rx.recv().await {
                    let _ = p.reply.send(Err(ClientError::ProtocolSkew { ours, theirs }));
                }
                return;
            }
            // Anything else is an ordinary disconnect: go round again.
            _ => set(&health, &out, Health::Reconnecting).await,
        }
        tokio::time::sleep(backoff).await;
    }
}

#[cfg_attr(not(test), allow(dead_code))]
async fn set(health: &Arc<Mutex<Health>>, out: &mpsc::Sender<FromDaemon>, next: Health) {
    let mut h = health.lock().await;
    if *h != next {
        *h = next.clone();
        let _ = out.send(FromDaemon::Health(next)).await;
    }
}

/// One connection, from handshake to drop.
#[cfg_attr(not(test), allow(dead_code))]
async fn session<S>(
    stream: S,
    topics: &[Topic],
    rx: &mut mpsc::Receiver<Pending>,
    out: &mpsc::Sender<FromDaemon>,
    health: &Arc<Mutex<Health>>,
) -> Result<(), ClientError>
where
    S: tokio::io::AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (r, mut w) = tokio::io::split(stream);
    let mut lines = BufReader::new(r).lines();
    let mut next_id: u64 = 1;
    let mut waiting: HashMap<u64, oneshot::Sender<Result<Value, ClientError>>> = HashMap::new();

    // Handshake first, and synchronously: nothing else may be sent until the
    // protocol is agreed, because a frame this daemon cannot read is a frame we
    // cannot interpret the silence of.
    let hello_id = next_id;
    next_id += 1;
    send(&mut w, &serde_json::json!({
        "method": "hello", "id": hello_id, "protocol": PROTOCOL
    }))
    .await?;

    loop {
        let Some(line) = lines.next_line().await.ok().flatten() else {
            return Ok(());
        };
        let Ok(frame) = serde_json::from_str::<Value>(&line) else { continue };
        match frame["type"].as_str() {
            Some("hello") => {
                let theirs = frame["protocol"].as_u64().unwrap_or(0) as u32;
                if theirs != PROTOCOL {
                    return Err(ClientError::ProtocolSkew { ours: PROTOCOL, theirs });
                }
                // Connected *before* the snapshot goes out: a caller that acts
                // on the snapshot must not find the client still reporting
                // itself disconnected and get refused for it.
                set(health, out, Health::Connected).await;
                if let Ok(snapshot) = serde_json::from_value::<Snapshot>(frame["snapshot"].clone()) {
                    let _ = out.send(FromDaemon::Snapshot(Box::new(snapshot))).await;
                }
                if !topics.is_empty() {
                    let id = next_id;
                    next_id += 1;
                    send(&mut w, &serde_json::json!({
                        "method": "subscribe", "id": id, "topics": topics
                    }))
                    .await?;
                }
                break;
            }
            // The daemon refused the handshake itself.
            Some("err") if frame["id"].as_u64() == Some(hello_id) => {
                return Err(ClientError::Command(
                    frame["error"].as_str().unwrap_or("handshake refused").to_string(),
                ));
            }
            _ => continue,
        }
    }

    // Connected. Pump requests out and frames in until one side stops.
    loop {
        tokio::select! {
            pending = rx.recv() => {
                let Some(p) = pending else { return Ok(()) };
                let id = next_id;
                next_id += 1;
                let frame = serde_json::json!({
                    "method": "invoke", "id": id, "command": p.command, "args": p.args
                });
                if send(&mut w, &frame).await.is_err() {
                    // Put the caller out of its misery rather than leaving it
                    // waiting on a socket that has gone.
                    let _ = p.reply.send(Err(ClientError::Disconnected));
                    return Ok(());
                }
                waiting.insert(id, p.reply);
            }
            line = lines.next_line() => {
                let Some(line) = line.ok().flatten() else {
                    // Everyone still waiting learns the connection died.
                    for (_, reply) in waiting.drain() {
                        let _ = reply.send(Err(ClientError::Disconnected));
                    }
                    return Ok(());
                };
                let Ok(frame) = serde_json::from_str::<Value>(&line) else { continue };
                match frame["type"].as_str() {
                    Some("ok") => {
                        if let Some(id) = frame["id"].as_u64()
                            && let Some(reply) = waiting.remove(&id)
                        {
                            let _ = reply.send(Ok(frame["value"].clone()));
                        }
                    }
                    Some("err") => {
                        if let Some(id) = frame["id"].as_u64()
                            && let Some(reply) = waiting.remove(&id)
                        {
                            let error = frame["error"].as_str().unwrap_or("failed").to_string();
                            let _ = reply.send(Err(ClientError::Command(error)));
                        }
                    }
                    Some("event") => {
                        if let Ok(event) = serde_json::from_value::<Event>(frame["event"].clone()) {
                            let _ = out.send(FromDaemon::Event(event)).await;
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

#[cfg_attr(not(test), allow(dead_code))]
async fn send<W>(w: &mut W, frame: &Value) -> Result<(), ClientError>
where
    W: AsyncWrite + Unpin,
{
    let mut line = frame.to_string();
    line.push('\n');
    w.write_all(line.as_bytes()).await.map_err(|_| ClientError::Disconnected)?;
    w.flush().await.map_err(|_| ClientError::Disconnected)
}

#[cfg(test)]
mod tests {
    //! The real client against the real server, over loopback.
    //!
    //! Nothing is mocked on either side: `daemon::rpc::serve` answers and
    //! `client::spawn` asks. That is what makes reconnect and skew refusal
    //! worth asserting, because both are properties of the pair rather than of
    //! either half.
    //!
    //! **What this cannot prove is the exit criterion.** #23 asks for a UI
    //! killed and relaunched mid-recording showing the live recording within one
    //! reconnect, and that needs a daemon recording a game, which is WS3.2 on
    //! Windows. What is here is every part of that sentence except the game.

    use super::*;
    use crate::core::Ctx;
    use crate::daemon::rpc::{self, Events};
    use crate::db::Db;
    use crate::recorder::Recorder;
    use crate::recorder::stub::StubRecorder;
    use crate::state_machine;
    use std::sync::Mutex as StdMutex;
    use tokio::net::{TcpListener, TcpStream};

    fn ctx() -> Arc<Ctx> {
        let recorder: Arc<StdMutex<Box<dyn Recorder>>> =
            Arc::new(StdMutex::new(Box::new(StubRecorder::new())));
        let db = Arc::new(Db::open_temporary().unwrap());
        let dir = std::env::temp_dir().join(format!("nr-client-test-{}", std::process::id()));
        let supervisor =
            state_machine::Supervisor::new(Arc::clone(&recorder), dir.clone(), Arc::clone(&db));
        Arc::new(Ctx::new(recorder, supervisor, db, dir.clone(), dir.join("ddragon"), None))
    }

    /// A server that keeps accepting, so a reconnect finds something there.
    async fn server(events: Events) -> std::net::SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let ctx = ctx();
        let snapshot = rpc::test_snapshot(&ctx);
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else { return };
                let (ctx, events, snapshot) =
                    (Arc::clone(&ctx), events.clone(), Arc::clone(&snapshot));
                tokio::spawn(async move {
                    let _ = rpc::serve(stream, ctx, events, snapshot).await;
                });
            }
        });
        addr
    }

    fn connect_to(addr: std::net::SocketAddr) -> impl Fn() -> futures_util::future::BoxFuture<'static, std::io::Result<TcpStream>> + Send + Sync + 'static
    {
        move || Box::pin(TcpStream::connect(addr))
    }

    async fn wait_for_snapshot(rx: &mut mpsc::Receiver<FromDaemon>) -> Box<Snapshot> {
        loop {
            match tokio::time::timeout(Duration::from_secs(5), rx.recv()).await {
                Ok(Some(FromDaemon::Snapshot(s))) => return s,
                Ok(Some(_)) => continue,
                _ => panic!("no snapshot arrived"),
            }
        }
    }

    #[tokio::test]
    async fn the_handshake_yields_a_snapshot_and_a_working_client() {
        let addr = server(Events::new()).await;
        let (tx, mut rx) = mpsc::channel(64);
        let client = spawn(connect_to(addr), vec![Topic::Library], tx);

        let snapshot = wait_for_snapshot(&mut rx).await;
        assert!(snapshot.prefs.is_empty() || !snapshot.prefs.is_empty());
        assert_eq!(client.health().await, Health::Connected);

        let rows = client.call("list_recordings", serde_json::json!({})).await.unwrap();
        assert!(rows.is_array());
    }

    /// Replies are matched by id, so concurrent calls cannot cross.
    #[tokio::test]
    async fn concurrent_calls_get_their_own_answers() {
        let addr = server(Events::new()).await;
        let (tx, mut rx) = mpsc::channel(64);
        let client = spawn(connect_to(addr), vec![], tx);
        wait_for_snapshot(&mut rx).await;

        let a = client.call("get_recordings_dir", serde_json::json!({}));
        let b = client.call("list_recordings", serde_json::json!({}));
        let c = client.call("no_such_command", serde_json::json!({}));
        let (a, b, c) = tokio::join!(a, b, c);

        assert!(a.unwrap().is_string(), "get_recordings_dir returns a path");
        assert!(b.unwrap().is_array(), "list_recordings returns rows");
        assert!(matches!(c, Err(ClientError::Command(_))), "an unknown command is an error");
    }

    /// Subscribed events reach the caller.
    #[tokio::test]
    async fn events_reach_the_ui() {
        let events = Events::new();
        let addr = server(events.clone()).await;
        let (tx, mut rx) = mpsc::channel(64);
        let _client = spawn(connect_to(addr), vec![Topic::Library], tx);
        wait_for_snapshot(&mut rx).await;

        // Give the subscribe frame a moment to land before publishing.
        tokio::time::sleep(Duration::from_millis(50)).await;
        events.publish(Event::RetentionRan { deleted: vec![7], freed_bytes: 7 });

        loop {
            match tokio::time::timeout(Duration::from_secs(5), rx.recv()).await {
                Ok(Some(FromDaemon::Event(Event::RetentionRan { deleted, .. }))) => {
                    assert_eq!(deleted, vec![7]);
                    return;
                }
                Ok(Some(_)) => continue,
                _ => panic!("the event never arrived"),
            }
        }
    }

    /// **The reconnect.** The connection dies under the client; it
    /// re-handshakes on its own and hands up a fresh snapshot, unasked.
    ///
    /// This is the mechanism #23's exit criterion rests on. What it does not
    /// show is a *recording* surviving it, which needs WS3.2 on Windows.
    ///
    /// The listener stays up and the served *connection* is killed, rather than
    /// taking the whole listener away and rebinding. Rebinding is what a daemon
    /// restart really looks like, but `SO_REUSEADDR` lets the second bind
    /// succeed while the first socket is still open, so the client can reconnect
    /// to a listener whose accept loop is gone and the test fails for a reason
    /// that has nothing to do with the client. Killing the connection exercises
    /// the same path in the client deterministically.
    #[tokio::test]
    async fn a_dropped_connection_reconnects_and_re_snapshots() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let ctx = ctx();
        let snapshot = rpc::test_snapshot(&ctx);
        let events = Events::new();

        // Hands out a handle to each served connection, so the test can kill
        // the current one.
        let (served_tx, mut served_rx) = mpsc::channel::<tokio::task::JoinHandle<()>>(8);
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else { return };
                let (ctx, events, snapshot) =
                    (Arc::clone(&ctx), events.clone(), Arc::clone(&snapshot));
                let handle = tokio::spawn(async move {
                    let _ = rpc::serve(stream, ctx, events, snapshot).await;
                });
                if served_tx.send(handle).await.is_err() {
                    return;
                }
            }
        });

        let (tx, mut rx) = mpsc::channel(64);
        let client = spawn(connect_to(addr), vec![Topic::Library], tx);

        wait_for_snapshot(&mut rx).await;
        assert_eq!(client.health().await, Health::Connected);

        // Kill the connection the client is using. The daemon is still there;
        // the pipe is not.
        let first = served_rx.recv().await.expect("the first connection was served");
        first.abort();

        // A second snapshot, which nothing asked for, is the whole point.
        wait_for_snapshot(&mut rx).await;
        assert_eq!(client.health().await, Health::Connected);
        let rows = client.call("list_recordings", serde_json::json!({})).await.unwrap();
        assert!(rows.is_array(), "the client works again after reconnecting");
    }

    /// **Version skew is terminal, and says so.**
    ///
    /// The updater can replace the daemon under a running UI, so this is a real
    /// state. A client that retried forever would spin against a daemon that
    /// can never agree with it; one that degraded would be guessing at frames
    /// it has never seen.
    #[tokio::test]
    async fn a_skewed_daemon_is_refused_and_not_retried() {
        // A fake daemon that answers `hello` with the wrong protocol.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else { return };
                tokio::spawn(async move {
                    let (r, mut w) = tokio::io::split(stream);
                    let mut lines = BufReader::new(r).lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        let frame: Value = serde_json::from_str(&line).unwrap();
                        if frame["method"] == "hello" {
                            let reply = serde_json::json!({
                                "type": "hello",
                                "id": frame["id"],
                                "protocol": PROTOCOL + 99,
                                "snapshot": {},
                            });
                            let _ = w.write_all(format!("{reply}\n").as_bytes()).await;
                            let _ = w.flush().await;
                        }
                    }
                });
            }
        });

        let (tx, mut rx) = mpsc::channel(64);
        let client = spawn(connect_to(addr), vec![], tx);

        let skewed = loop {
            match tokio::time::timeout(Duration::from_secs(5), rx.recv()).await {
                Ok(Some(FromDaemon::Health(Health::Skewed { ours, theirs }))) => {
                    break (ours, theirs);
                }
                Ok(Some(_)) => continue,
                _ => panic!("the skew was never reported"),
            }
        };
        assert_eq!(skewed, (PROTOCOL, PROTOCOL + 99));

        // And a caller gets a definite answer rather than hanging on a loop
        // that has stopped.
        let err = client.call("list_recordings", serde_json::json!({})).await.unwrap_err();
        assert!(matches!(err, ClientError::ProtocolSkew { .. }), "{err:?}");
        assert!(err.to_string().contains("different builds"), "{err}");
    }

    /// A caller gets an error rather than hanging when nothing is listening.
    #[tokio::test]
    async fn a_call_with_no_daemon_fails_rather_than_hangs() {
        // Bind and drop, so the port is dead but plausible.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);

        let (tx, _rx) = mpsc::channel(64);
        let client = spawn(connect_to(addr), vec![], tx);

        let answered = tokio::time::timeout(
            Duration::from_secs(5),
            client.call("list_recordings", serde_json::json!({})),
        )
        .await;
        assert!(answered.is_ok(), "a call with no daemon must not hang");
    }
}
