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
use crate::contract::snapshot::Snapshot;
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
    /// The handshake's answer, carrying the whole of the daemon's observable
    /// state.
    ///
    /// **The snapshot rides on `hello` rather than being fetched after it.** A
    /// client that connects, or reconnects after a dropped pipe, gets one
    /// snapshot and then a stream of events; it never replays history. Making
    /// that a second round trip would open a window between the two where
    /// events arrive that the client has no baseline to apply them to, which is
    /// the exact gap `seq` exists to close.
    #[serde(rename_all = "camelCase")]
    Hello { id: u64, protocol: u32, snapshot: Box<Snapshot> },
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

    /// How many sessions are attached. See `snapshot::Stream::has_subscribers`
    /// for what asks and why.
    pub fn subscriber_count(&self) -> usize {
        self.0.receiver_count()
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

/// Builds the snapshot a `hello` answers with.
///
/// A closure rather than something assembled here, because `Snapshot::assemble`
/// needs the last observed LCU status and the stream's position, and neither
/// belongs to the transport: the daemon owns the gameflow watcher and the event
/// counter. WS2.4 declared those two fields as supplied for the same reason.
pub type SnapshotSource = Arc<dyn Fn() -> Snapshot + Send + Sync>;

/// Serves one connection until it closes or the protocol is violated.
///
/// Generic over the stream so the same code serves a Windows named pipe in
/// production and a Unix socket in the tests. That is not a convenience: the
/// alternative is a protocol whose only exercise is on the Windows box, which
/// is the loop this project is organised to stay out of.
pub async fn serve<S>(
    stream: S,
    ctx: Arc<Ctx>,
    events: Events,
    snapshot: SnapshotSource,
) -> std::io::Result<()>
where
    S: tokio::io::AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (rx, mut tx) = tokio::io::split(stream);
    let mut lines = BufReader::new(rx).lines();

    // Subscribed from the start, not from `subscribe`: a client that asks for
    // topics should not miss what happened between its `hello` and its
    // subscription. Frames for topics it has not asked for are dropped on the
    // way out, which costs nothing and closes that window.
    let mut rx_events = events.subscribe();
    let mut session = Session { topics: Vec::new(), greeted: false };

    // **One task owns the connection.** The first version spawned the event
    // pump separately, sharing the write half through an `Arc<Mutex<..>>`. That
    // is a leak with teeth: aborting the serve task does not run its cleanup,
    // so the pump outlived it still holding the socket, and a client whose
    // daemon had gone away never saw EOF and never reconnected. Selecting in
    // one task means dropping it drops everything, and the writer needs no lock
    // because there is only ever one writer.
    loop {
        tokio::select! {
            line = lines.next_line() => {
                let Some(line) = line? else { return Ok(()) };
                if line.trim().is_empty() {
                    continue;
                }
                let reply = match serde_json::from_str::<Request>(&line) {
                    Ok(request) => handle(request, &ctx, &mut session, &snapshot).await,
                    // A frame we cannot parse has no id to echo, so the error
                    // goes out under `NO_ID`. Answering at all is deliberate:
                    // silence looks identical to a hung daemon from the far end.
                    Err(e) => Reply::Err { id: NO_ID, error: format!("malformed frame: {e}") },
                };
                if write_frame(&mut tx, &reply).await.is_err() {
                    return Ok(());
                }
            }
            received = rx_events.recv() => {
                let reply = match received {
                    Ok(event) => {
                        if !(session.greeted && session.topics.contains(&event.topic())) {
                            continue;
                        }
                        Reply::Event { event }
                    }
                    Err(broadcast::error::RecvError::Lagged(dropped)) => {
                        // The contract declares this for exactly this moment.
                        Reply::Event { event: Event::Lagged { dropped: dropped as u32 } }
                    }
                    Err(broadcast::error::RecvError::Closed) => return Ok(()),
                };
                if write_frame(&mut tx, &reply).await.is_err() {
                    return Ok(());
                }
            }
        }
    }
}

async fn handle(
    request: Request,
    ctx: &Arc<Ctx>,
    session: &mut Session,
    snapshot: &SnapshotSource,
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
            session.greeted = true;
            Reply::Hello { id, protocol: PROTOCOL, snapshot: Box::new(snapshot()) }
        }
        Request::Subscribe { id, topics } => {
            if !session.greeted {
                return Reply::Err { id, error: "subscribe before hello".to_string() };
            }
            session.topics = topics;
            Reply::Ok { id, value: Value::Null }
        }
        Request::Invoke { id, command, args } => {
            if !session.greeted {
                return Reply::Err { id, error: "invoke before hello".to_string() };
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
/// No lock, because there is exactly one writer: replies and events are written
/// from the same task, so two frames cannot interleave halves into one line.
async fn write_frame<W>(w: &mut W, reply: &Reply) -> std::io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    let mut line = serde_json::to_string(reply).map_err(std::io::Error::other)?;
    line.push('\n');
    w.write_all(line.as_bytes()).await?;
    w.flush().await
}

// --- Where the daemon listens, and how a client reaches it ----------------
//
// The transport is a Windows named pipe in production and a Unix socket on a
// dev box. Both sides of the split read the address from here — the daemon to
// bind it, the UI to connect to it — because a pipe name that two modules
// spell separately is a pipe name they can disagree about, and the failure
// looks exactly like a daemon that is not running.

/// Which build this is, as it appears in the endpoint name.
///
/// `tauri.devtools.conf.json` overrides `productName` but **not**
/// `identifier`, so a devtools build and a release build already share
/// `app_data_dir()`, the database and the recordings folder. One process can
/// survive that. Two daemons cannot: they would bind the same pipe, and
/// whichever started first would silently own the other's clients — a dev
/// portal driving the release daemon's recorder, or the reverse. Scoping the
/// name by build identity is what keeps them apart (implementation plan §4.2).
const BUILD: &str = if cfg!(feature = "devtools") { "devtools" } else { "release" };

/// The address the daemon binds and a client connects to.
///
/// `data_dir` is the app data directory, and is used on Unix only — a named
/// pipe lives in the kernel's namespace rather than the filesystem, so on
/// Windows there is nothing to put it beside.
pub fn endpoint(data_dir: &std::path::Path) -> std::path::PathBuf {
    #[cfg(windows)]
    {
        let _ = data_dir;
        // `\\.\pipe\` is the only namespace named pipes live in, and the name
        // after it is flat: no directories, and it may not contain a
        // backslash. `IDENTIFIER` carries dots, which are fine.
        std::path::PathBuf::from(format!(
            r"\\.\pipe\ninja-recorder.{}.{BUILD}",
            crate::daemon::IDENTIFIER
        ))
    }
    #[cfg(unix)]
    {
        // Beside the database rather than in `/tmp`: the socket is per-user
        // state, `$TMPDIR` is world-writable, and a path under the app data
        // directory inherits that directory's permissions. Unix sockets have a
        // ~108-byte path limit, which this is comfortably inside.
        data_dir.join(format!("daemon.{BUILD}.sock"))
    }
}

/// The accepting half of the endpoint.
///
/// Also the daemon's single-instance guard, which is why `bind` distinguishes
/// "already running" from "failed": see its doc comment.
pub struct Listener {
    #[cfg(windows)]
    name: std::ffi::OsString,
    /// The idle server instance, waiting for the next client. `accept` hands
    /// this one over and immediately creates its replacement, so the name is
    /// owned continuously — the moment no instance exists is the moment
    /// another process could take the name.
    #[cfg(windows)]
    idle: Option<tokio::net::windows::named_pipe::NamedPipeServer>,
    #[cfg(unix)]
    listener: tokio::net::UnixListener,
    #[cfg(unix)]
    path: std::path::PathBuf,
}

impl Listener {
    /// Binds the endpoint, or reports that a daemon already owns it.
    ///
    /// `Ok(None)` means **another daemon is already running**, which is a
    /// normal outcome and not an error: the plan's startup rule 3 is that a
    /// second `--daemon` launch exits 0 silently rather than signalling the
    /// first, because the first might be recording.
    ///
    /// ## This is the single-instance check, and there is no separate mutex
    ///
    /// The plan sketches a named mutex alongside the pipe. One lock is better
    /// than two here, because the thing worth protecting is the *endpoint*:
    /// a mutex held while the pipe failed to bind, or a pipe bound while the
    /// mutex was somehow free, are both states where the answer to "is a
    /// daemon running" depends on which one you asked. Binding the endpoint
    /// answers it directly, on both platforms, and needs no Win32 surface
    /// beyond what tokio already wraps.
    ///
    /// Windows gets that from `first_pipe_instance`, which fails with
    /// `ERROR_ACCESS_DENIED` when another process already has a server on the
    /// name. Unix gets it from `bind`, plus a probe: a socket file outlives
    /// the process that made it, so `EADDRINUSE` alone cannot tell a live
    /// daemon from a crashed one's leftovers. Connecting is what separates
    /// them — someone answers, or nobody does and the file is stale.
    pub fn bind(endpoint: &std::path::Path) -> std::io::Result<Option<Listener>> {
        #[cfg(windows)]
        {
            let name = endpoint.as_os_str().to_os_string();
            let mut security = crate::daemon::pipe_acl::PipeSecurity::current_user_only();
            match create_instance(&name, true, security.as_mut()) {
                Ok(idle) => Ok(Some(Listener { name, idle: Some(idle) })),
                // The one error that means "someone else got here first".
                // Every other failure is a real one and is reported.
                Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => Ok(None),
                Err(e) => Err(e),
            }
        }
        #[cfg(unix)]
        {
            use tokio::net::UnixListener;

            match UnixListener::bind(endpoint) {
                Ok(listener) => {
                    Ok(Some(Listener { listener, path: endpoint.to_path_buf() }))
                }
                Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                    // Someone answering means a live daemon. A refused
                    // connection means the file outlived the process that
                    // bound it — a crash, or a kill -9 — and nothing is
                    // listening, so it is ours to remove.
                    match std::os::unix::net::UnixStream::connect(endpoint) {
                        Ok(_) => Ok(None),
                        Err(_) => {
                            std::fs::remove_file(endpoint)?;
                            let listener = UnixListener::bind(endpoint)?;
                            Ok(Some(Listener { listener, path: endpoint.to_path_buf() }))
                        }
                    }
                }
                Err(e) => Err(e),
            }
        }
    }

    /// Waits for the next client.
    ///
    /// The returned stream is a `NamedPipeServer` on Windows and a
    /// `UnixStream` on Unix; `serve` is generic over both, which is the whole
    /// reason the protocol can be exercised on a dev box.
    pub async fn accept(&mut self) -> std::io::Result<impl ClientStream + use<>> {
        #[cfg(windows)]
        {
            // Rebuilt here rather than assumed, because either line below can
            // leave us without one: a failed `connect` returns before the
            // replacement is made, and the caller's answer to that is to call
            // `accept` again. An `expect` on the `Option` would turn one
            // refused connection into a panicking daemon on the next client.
            //
            // The DACL is rebuilt per instance rather than kept on the
            // `Listener`: every instance of a named pipe carries its own
            // security descriptor, and one shared `SECURITY_ATTRIBUTES` handed
            // out as a `*mut` from several places is the kind of aliasing this
            // is not worth risking to save a SID lookup.
            //
            // **Each one is scoped so it never crosses the `await` below, and
            // that is load-bearing rather than tidy.** `PipeSecurity` owns raw
            // pointers and so is not `Send`; a non-`Send` value held across an
            // await makes the whole future non-`Send`, and `accept` is called
            // from a `tokio::spawn`. The descriptor is only needed for the
            // length of the create call anyway, because Windows copies it into
            // the pipe object. Found by CI, which is the only thing that
            // compiles this branch.
            let idle = match self.idle.take() {
                Some(idle) => idle,
                None => {
                    let mut security = crate::daemon::pipe_acl::PipeSecurity::current_user_only();
                    create_instance(&self.name, false, security.as_mut())?
                }
            };
            idle.connect().await?;

            // The replacement, made while the connected instance is still in
            // hand, so the name is owned continuously. Best-effort on purpose:
            // a client is already connected, and failing the accept to report
            // that the *next* instance could not be made would drop a session
            // over a problem the next `accept` will retry anyway.
            {
                let mut security = crate::daemon::pipe_acl::PipeSecurity::current_user_only();
                self.idle = create_instance(&self.name, false, security.as_mut()).ok();
            }
            Ok(idle)
        }
        #[cfg(unix)]
        {
            self.listener.accept().await.map(|(stream, _)| stream)
        }
    }
}

/// A socket file is not cleaned up by the kernel, so the daemon cleans up
/// after itself. A crash still leaves one behind, which is what `bind`'s probe
/// is for; this only spares the ordinary case.
#[cfg(unix)]
impl Drop for Listener {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Creates one instance of the named pipe.
///
/// The single place `ServerOptions` is configured, so the security properties
/// cannot differ between the first instance and its replacements: a pipe whose
/// second instance was created with a weaker ACL would be a hole open from the
/// second client onward, which is exactly the kind of thing that never shows up
/// in testing.
///
/// `security` is `None` when the descriptor could not be built, in which case
/// the pipe gets the process's default DACL. `pipe_acl` has already said so in
/// the log.
#[cfg(windows)]
fn create_instance(
    name: &std::ffi::OsStr,
    first: bool,
    security: Option<&mut crate::daemon::pipe_acl::PipeSecurity>,
) -> std::io::Result<tokio::net::windows::named_pipe::NamedPipeServer> {
    use tokio::net::windows::named_pipe::ServerOptions;

    let mut options = ServerOptions::new();
    options
        .first_pipe_instance(first)
        // Default, set explicitly because it is a security property rather than
        // a tuning one: a pipe reachable over SMB would let a machine on the
        // network drive this one's recorder.
        .reject_remote_clients(true);

    match security {
        // SAFETY: `as_ptr` hands back a pointer to a `SECURITY_ATTRIBUTES` that
        // `PipeSecurity` owns and keeps alive for the whole call, pointing at a
        // descriptor it also owns. Windows copies both into the pipe object, so
        // neither has to outlive this line.
        Some(security) => unsafe {
            options.create_with_security_attributes_raw(name, security.as_ptr())
        },
        None => options.create(name),
    }
}

/// What both ends of the transport require of a stream. Named so `accept` can
/// return one type on Windows and another on Unix without every caller
/// repeating the bounds.
pub trait ClientStream: tokio::io::AsyncRead + AsyncWrite + Unpin + Send + 'static {}
impl<T: tokio::io::AsyncRead + AsyncWrite + Unpin + Send + 'static> ClientStream for T {}

/// Connects to a running daemon.
///
/// The client half of `endpoint`, here rather than in `ui` so the two spellings
/// of the address cannot drift: the same file binds it and opens it.
pub async fn connect(endpoint: &std::path::Path) -> std::io::Result<impl ClientStream + use<>> {
    #[cfg(windows)]
    {
        // `ClientOptions::open` is synchronous and can fail with
        // `ERROR_PIPE_BUSY` when every instance is momentarily taken. The
        // caller's answer to a failed connect is a backoff retry, which is
        // also the answer to a busy pipe, so this does not special-case it.
        tokio::net::windows::named_pipe::ClientOptions::new().open(endpoint)
    }
    #[cfg(unix)]
    {
        tokio::net::UnixStream::connect(endpoint).await
    }
}

/// A snapshot source for tests. The daemon's real one is `daemon::snapshot`.
///
/// Lives here rather than in either test module because both `rpc`'s tests and
/// `ui::client`'s drive a real server, and two copies of the same stand-in
/// would be one more thing able to disagree.
#[cfg(test)]
pub(crate) fn test_snapshot(ctx: &Arc<Ctx>) -> SnapshotSource {
    let ctx = Arc::clone(ctx);
    Arc::new(move || {
        Snapshot::assemble(
            &ctx,
            0,
            crate::core::LcuStatus { connected: false, phase: None, summoner: None, error: None },
        )
    })
}

#[cfg(test)]
mod tests {
    //! Over a loopback socket, which is the point.
    //!
    //! The production transport is a Windows named pipe, and a protocol whose
    //! only exercise is on the Windows box is one that gets tested once a week.
    //! `serve` is generic over the stream precisely so the same code can be
    //! driven here in milliseconds.
    //!
    //! **TCP on loopback rather than a Unix socket, and that is a correction.**
    //! #20 asks for a Unix socket, which is the right instinct: keep the
    //! protocol in the seconds-long dev loop instead of on the Windows box. But
    //! CI runs on `windows-latest` and nothing else (§9), so Unix-only tests
    //! would never run there at all, which is the same hole the other way
    //! round. Loopback runs in both places. `a_unix_socket_serves_the_same_code`
    //! below keeps the literal ask, gated to where it compiles.

    use super::*;
    use crate::core::Ctx;
    use crate::db::Db;
    use crate::recorder::Recorder;
    use crate::recorder::stub::StubRecorder;
    use crate::state_machine;
    use std::sync::Mutex;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::{TcpListener, TcpStream};

    fn ctx() -> Arc<Ctx> {
        let recorder: Arc<Mutex<Box<dyn Recorder>>> =
            Arc::new(Mutex::new(Box::new(StubRecorder::new())));
        let db = Arc::new(Db::open_temporary().unwrap());
        let dir = std::env::temp_dir().join(format!("nr-rpc-test-{}", std::process::id()));
        let supervisor =
            state_machine::Supervisor::new(Arc::clone(&recorder), dir.clone(), Arc::clone(&db));
        Arc::new(Ctx::new(recorder, supervisor, db, dir.clone(), dir.join("ddragon"), None))
    }

    type Rx = BufReader<tokio::net::tcp::OwnedReadHalf>;
    type Tx = tokio::net::tcp::OwnedWriteHalf;

    /// Spins up a server on a loopback port and returns a connected client.
    ///
    /// Port 0 so the OS picks a free one: two tests running concurrently must
    /// not be able to collide on a fixed number.
    async fn connected(events: Events) -> (Rx, Tx) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let ctx = ctx();
        let snapshot = super::test_snapshot(&ctx);
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let _ = serve(stream, ctx, events, snapshot).await;
        });

        let client = TcpStream::connect(addr).await.unwrap();
        let (rx, tx) = client.into_split();
        (BufReader::new(rx), tx)
    }

    async fn send(tx: &mut Tx, frame: &str) {
        tx.write_all(frame.as_bytes()).await.unwrap();
        tx.write_all(b"\n").await.unwrap();
        tx.flush().await.unwrap();
    }

    async fn next(rx: &mut Rx) -> Value {
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
        // The whole of the daemon's observable state, in the handshake: a
        // reconnecting client must not need a second round trip to have a
        // baseline for the events that follow.
        assert!(reply["snapshot"].is_object(), "hello must carry the snapshot: {reply}");
        assert!(reply["snapshot"]["state"].is_string());
        assert!(reply["snapshot"]["prefs"].is_object());
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

    /// #20's literal ask, kept: the same `serve` over a Unix socket.
    ///
    /// One test rather than the whole module, because the stream type is not
    /// what the protocol tests are about. What this proves is that `serve` is
    /// genuinely generic, which is the claim the Windows named pipe rests on.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_unix_socket_serves_the_same_code() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        use tokio::net::{UnixListener, UnixStream};

        let path = std::env::temp_dir()
            .join(format!("nr-rpc-{}-unix.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();

        let ctx = ctx();
        let snapshot = super::test_snapshot(&ctx);
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let _ = serve(stream, ctx, Events::new(), snapshot).await;
        });

        let client = UnixStream::connect(&path).await.unwrap();
        let _ = std::fs::remove_file(&path);
        let (rx, mut tx) = client.into_split();
        let mut rx = BufReader::new(rx);

        tx.write_all(b"{\"method\":\"hello\",\"id\":1,\"protocol\":1}\n").await.unwrap();
        tx.flush().await.unwrap();

        let mut line = String::new();
        rx.read_line(&mut line).await.unwrap();
        let reply: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(reply["type"], "hello");
        assert_eq!(reply["protocol"], PROTOCOL);
    }

    // --- the endpoint ------------------------------------------------------
    //
    // These drive the *production* transport — a named pipe on Windows, a Unix
    // socket on a dev box — rather than the loopback socket the protocol tests
    // above use. That is the point of them: `serve` being generic is what lets
    // the protocol be tested in milliseconds, and this is the part that proves
    // the generic ends up connected to something real.

    /// A unique endpoint, because these tests run concurrently and, on
    /// Windows, in a namespace shared with any daemon already running on the
    /// machine. Binding `endpoint()` itself in a test would either collide
    /// with another test or, worse, take the name a real daemon was about to
    /// want.
    fn test_endpoint() -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let unique = format!("{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed));

        #[cfg(windows)]
        {
            std::path::PathBuf::from(format!(r"\\.\pipe\ninja-recorder-test.{unique}"))
        }
        #[cfg(unix)]
        {
            let dir = std::env::temp_dir().join(format!("nr-endpoint-test-{unique}"));
            std::fs::create_dir_all(&dir).unwrap();
            dir.join("daemon.sock")
        }
    }

    /// The name carries the identifier *and* the build, because a devtools
    /// build and a release build share `app_data_dir()` and would otherwise
    /// share this too — a dev portal driving the release daemon's recorder.
    #[test]
    fn the_endpoint_is_scoped_by_build_identity() {
        let endpoint = endpoint(std::path::Path::new("/tmp/data"));
        let name = endpoint.display().to_string();

        assert!(name.contains(BUILD), "the endpoint must name the build: {name}");
        assert_eq!(
            BUILD,
            if cfg!(feature = "devtools") { "devtools" } else { "release" },
            "the two builds must not agree on a name"
        );

        #[cfg(windows)]
        {
            assert!(name.starts_with(r"\\.\pipe\"), "a named pipe lives in the pipe namespace: {name}");
            assert!(name.contains(crate::daemon::IDENTIFIER), "the endpoint must name the app: {name}");
        }
        #[cfg(unix)]
        {
            // Under the data directory, which is per-user, rather than in a
            // world-writable `/tmp`.
            assert!(endpoint.starts_with("/tmp/data"), "the socket belongs beside the database: {name}");
        }
    }

    /// The single-instance rule, and the reason `bind` returns an `Option`: a
    /// second daemon must find the endpoint owned and leave, rather than
    /// racing the first for it or signalling it to quit while it records.
    #[tokio::test]
    async fn a_second_bind_finds_the_endpoint_already_owned() {
        let endpoint = test_endpoint();
        let first = Listener::bind(&endpoint).unwrap();
        assert!(first.is_some(), "the first daemon takes the endpoint");

        let second = Listener::bind(&endpoint).unwrap();
        assert!(second.is_none(), "the second must find it owned, not fail and not steal it");

        // And once the first lets go, the name is available again — otherwise
        // a daemon could not be restarted without a reboot.
        drop(first);
        let third = Listener::bind(&endpoint).unwrap();
        assert!(third.is_some(), "the endpoint is free once its owner is gone");
    }

    /// A socket file outlives the process that bound it, so "the file is
    /// there" cannot mean "a daemon is running". A daemon that believed it
    /// would refuse to start after a single crash, forever, with nothing to
    /// tell the user but silence.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_socket_left_behind_by_a_dead_daemon_is_taken_over() {
        let endpoint = test_endpoint();

        // Exactly what a kill -9 leaves: the file, and nothing listening.
        let stale = std::os::unix::net::UnixListener::bind(&endpoint).unwrap();
        drop(stale);
        assert!(endpoint.exists(), "dropping a listener leaves the file behind");

        let listener = Listener::bind(&endpoint).unwrap();
        assert!(listener.is_some(), "a stale socket is not a running daemon");
    }

    /// The exit criterion, in a test: a client connects over the transport the
    /// daemon actually listens on, hands it a command, and gets the answer.
    ///
    /// On Windows that is the named pipe, which is what CI runs; on a dev box
    /// it is the Unix socket. Both go through `connect`, which is the same
    /// function the UI will use to find the daemon (WS3.5).
    #[tokio::test]
    async fn a_client_reaches_the_daemon_over_the_real_endpoint() {
        let endpoint = test_endpoint();
        let mut listener = Listener::bind(&endpoint).unwrap().expect("a fresh endpoint is free");

        let ctx = ctx();
        let snapshot = super::test_snapshot(&ctx);
        let events = Events::new();
        let published = events.clone();
        tokio::spawn(async move {
            let stream = listener.accept().await.unwrap();
            let _ = serve(stream, ctx, events, snapshot).await;
            // Held until the session ends: dropping the listener early would
            // release the name while a client is still connected.
            drop(listener);
        });

        let stream = connect(&endpoint).await.expect("the daemon is listening");
        let (rx, mut tx) = tokio::io::split(stream);
        let mut rx = BufReader::new(rx);

        // Every read is bounded. A frame that never arrives is a real failure
        // mode of a transport test — the wrong topic, a listener that dropped
        // the name — and an unbounded `read_line` reports it as a suite that
        // hangs forever rather than as a test that failed.
        async fn reply<R: tokio::io::AsyncBufRead + Unpin>(rx: &mut R) -> Value {
            let mut line = String::new();
            tokio::time::timeout(std::time::Duration::from_secs(10), rx.read_line(&mut line))
                .await
                .expect("the daemon answered within ten seconds")
                .unwrap();
            serde_json::from_str(&line).unwrap()
        }

        async fn send<W: AsyncWrite + Unpin>(tx: &mut W, frame: &str) {
            tx.write_all(frame.as_bytes()).await.unwrap();
            tx.write_all(b"\n").await.unwrap();
            tx.flush().await.unwrap();
        }

        send(&mut tx, r#"{"method":"hello","id":1,"protocol":1}"#).await;
        let hello = reply(&mut rx).await;
        assert_eq!(hello["type"], "hello", "the handshake crosses the real transport: {hello}");
        assert!(hello["snapshot"].is_object());

        // `daemon` as well as `recording`, because the event below is the
        // transport's own rather than a game's.
        send(&mut tx, r#"{"method":"subscribe","id":2,"topics":["recording","daemon"]}"#).await;
        assert_eq!(reply(&mut rx).await["type"], "ok");

        // A command, because a transport that carries only the handshake
        // carries nothing worth having.
        send(&mut tx, r#"{"method":"invoke","id":3,"command":"game_state_status","args":{}}"#).await;
        let ran = reply(&mut rx).await;
        assert_eq!(ran["id"], 3);
        assert_eq!(ran["value"]["state"], "Idle", "the command ran in the daemon: {ran}");

        // And an event pushed the other way, which is the half a
        // request/reply transport would let you forget about.
        published.publish(Event::Lagged { dropped: 7 });
        let event = reply(&mut rx).await;
        assert_eq!(event["type"], "event");
        assert_eq!(event["event"]["dropped"], 7);
    }
}
