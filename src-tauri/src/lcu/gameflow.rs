//! Gameflow phase tracking: LCU WebSocket subscription with a polling
//! fallback if the socket can't be established. DEVELOPMENT.md §3.1, §3.4.
//!
//! `watch` is driven continuously by the state machine's supervisor
//! (`state_machine::supervisor`), which owns the lockfile-change lifecycle
//! (when to start/stop/restart it). Not verified against a real LCU
//! connection yet — no League client is installed on the machine this was
//! written on (DEVELOPMENT.md §9).

use crate::{info, warn};
use super::client::{basic_auth_header, LcuClientError, LcuHttpClient};
use super::lockfile::LockfileInfo;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;
use tokio_tungstenite::tungstenite::Message;

/// League's gameflow phases. `Unknown` is a deliberate catch-all so an
/// unrecognized value from a client update never breaks parsing — we'd
/// rather surface an odd phase name than crash the watcher.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ts_rs::TS)]
pub enum GameflowPhase {
    None,
    Lobby,
    Matchmaking,
    CheckedIntoTournament,
    ReadyCheck,
    ChampSelect,
    GameStart,
    FailedToLaunch,
    InProgress,
    Reconnect,
    WaitingForStats,
    PreEndOfGame,
    EndOfGame,
    TerminatedInError,
    Unknown(String),
}

impl From<&str> for GameflowPhase {
    fn from(s: &str) -> Self {
        match s {
            "None" => GameflowPhase::None,
            "Lobby" => GameflowPhase::Lobby,
            "Matchmaking" => GameflowPhase::Matchmaking,
            "CheckedIntoTournament" => GameflowPhase::CheckedIntoTournament,
            "ReadyCheck" => GameflowPhase::ReadyCheck,
            "ChampSelect" => GameflowPhase::ChampSelect,
            "GameStart" => GameflowPhase::GameStart,
            "FailedToLaunch" => GameflowPhase::FailedToLaunch,
            "InProgress" => GameflowPhase::InProgress,
            "Reconnect" => GameflowPhase::Reconnect,
            "WaitingForStats" => GameflowPhase::WaitingForStats,
            "PreEndOfGame" => GameflowPhase::PreEndOfGame,
            "EndOfGame" => GameflowPhase::EndOfGame,
            "TerminatedInError" => GameflowPhase::TerminatedInError,
            other => GameflowPhase::Unknown(other.to_string()),
        }
    }
}

impl<'de> Deserialize<'de> for GameflowPhase {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(GameflowPhase::from(s.as_str()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum GameflowSource {
    WebSocket,
    Polling,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GameflowUpdate {
    pub phase: GameflowPhase,
    pub source: GameflowSource,
}

#[derive(Debug, thiserror::Error)]
pub enum GameflowError {
    #[error("websocket error: {0}")]
    WebSocket(#[from] Box<tokio_tungstenite::tungstenite::Error>),
    #[error("tls error: {0}")]
    Tls(#[from] native_tls::Error),
    #[error("invalid auth header: {0}")]
    Header(#[from] tokio_tungstenite::tungstenite::http::header::InvalidHeaderValue),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

/// What one connection to the LCU event socket saw, for the line logged when
/// it closes.
///
/// The socket lives two to three minutes and is then re-established, which was
/// invisible until #142 made the reconnect announce itself, and ambiguous
/// afterwards: a stream that simply ends returns `Ok(())` and said nothing at
/// all, so a clean close and a socket that was still up looked identical in
/// the log (#146).
///
/// Counting frame kinds rather than logging each one is deliberate. The
/// question is whether the client is dropping us and why, which is a property
/// of a whole connection, and one line per frame at 1 Hz would bury the answer
/// in the thing it is meant to explain.
#[derive(Debug, Default, PartialEq)]
struct FrameTally {
    text: u32,
    binary: u32,
    ping: u32,
    pong: u32,
    /// The close frame's code and reason, if the peer sent one. **This is the
    /// answer to #146's question** and it used to be discarded: the read loop
    /// matched `Message::Text` and let everything else fall through, so a
    /// deliberate close carrying `1000` and a reason read the same as a socket
    /// that vanished.
    close: Option<String>,
}

impl FrameTally {
    fn count(&mut self, msg: &Message) {
        match msg {
            Message::Text(_) => self.text += 1,
            Message::Binary(_) => self.binary += 1,
            Message::Ping(_) => self.ping += 1,
            Message::Pong(_) => self.pong += 1,
            Message::Close(frame) => {
                self.close = Some(match frame {
                    Some(f) => format!("code {}, {:?}", u16::from(f.code), f.reason.as_str()),
                    // A close with no payload. Legal, and less informative.
                    None => "no code".to_string(),
                });
            }
            // Raw frames are never produced on the read path.
            Message::Frame(_) => {}
        }
    }
}

impl std::fmt::Display for FrameTally {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} text, {} binary, {} ping, {} pong; {}",
            self.text,
            self.binary,
            self.ping,
            self.pong,
            // Pongs are tungstenite's business, not ours: it queues a reply to
            // every ping and flushes it from inside `read`, which is what
            // `ws.next()` drives. So a non-zero ping count with no manual
            // write from us is the socket being kept alive correctly, not the
            // starvation #146 guessed at.
            match &self.close {
                Some(why) => format!("peer closed ({why})"),
                None => "the stream ended without a close frame".to_string(),
            }
        )
    }
}

/// Watches gameflow phase changes until the caller's task is aborted.
/// Prefers the LCU WebSocket event stream (near-instant); if the socket
/// can't be established or drops, falls back to polling `http` on
/// `poll_interval` so the app still tracks phase changes, just less
/// promptly. Retries the WebSocket periodically rather than polling
/// forever, since the client may only have been slow to open the socket.
pub async fn watch<F>(
    lockfile: &LockfileInfo,
    http: &LcuHttpClient,
    poll_interval: Duration,
    mut on_update: F,
) where
    F: FnMut(GameflowUpdate) + Send,
{
    loop {
        if let Err(e) = watch_via_websocket(lockfile, http, &mut on_update).await {
            warn!("lcu", "websocket unavailable ({e}), falling back to polling");
            watch_via_polling(http, poll_interval, &mut on_update).await;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

async fn watch_via_polling<F>(http: &LcuHttpClient, interval: Duration, on_update: &mut F)
where
    F: FnMut(GameflowUpdate) + Send,
{
    let mut last: Option<GameflowPhase> = None;
    let mut ticker = tokio::time::interval(interval);
    loop {
        ticker.tick().await;
        match http
            .get_json::<GameflowPhase>("/lol-gameflow/v1/gameflow-phase")
            .await
        {
            Ok(phase) => {
                if last.as_ref() != Some(&phase) {
                    on_update(GameflowUpdate {
                        phase: phase.clone(),
                        source: GameflowSource::Polling,
                    });
                    last = Some(phase);
                }
            }
            // Client likely gone — stop polling and let the caller's
            // lockfile watch notice and re-drive discovery.
            Err(_) => return,
        }
    }
}

/// Subscribes to phase-change events, **and reads the phase we are
/// already in**.
///
/// The read is not optional. The socket only ever delivers *changes*, so a
/// watch that starts mid-game learns nothing until the phase next moves —
/// and the phase will never change *to* `InProgress` again this game. That
/// made the state machine's own recovery path unreachable: after any
/// mid-game restart of this watch, `ClientRunning` never advanced to
/// `WaitingForGame`, so the rest of the game went unrecorded (#75). It
/// also meant starting the app during a game recorded nothing until the
/// next one.
async fn watch_via_websocket<F>(
    lockfile: &LockfileInfo,
    http: &LcuHttpClient,
    on_update: &mut F,
) -> Result<(), GameflowError>
where
    F: FnMut(GameflowUpdate) + Send,
{
    let mut request = lockfile
        .ws_url()
        .into_client_request()
        .map_err(Box::new)?;
    request.headers_mut().insert(
        AUTHORIZATION,
        basic_auth_header(&lockfile.password).parse()?,
    );

    let connector = native_tls::TlsConnector::builder()
        .danger_accept_invalid_certs(true)
        .build()?;

    let (mut ws, _response) = tokio_tungstenite::connect_async_tls_with_config(
        request,
        None,
        false,
        Some(tokio_tungstenite::Connector::NativeTls(connector)),
    )
    .await
    .map_err(Box::new)?;

    // **Said out loud, because the alternative is a log that cannot answer the
    // question.** Only the failure was ever logged, so one "websocket
    // unavailable" line could mean the socket never worked or that it worked
    // for an hour and the client hung up on the way out, and nothing
    // distinguished them. That ambiguity is the whole of #94: a
    // `tokio-tungstenite` bump moved six minor versions, no test touches the
    // socket, and the only evidence a real session leaves is this file's
    // logging.
    info!("lcu", "connected to the LCU event socket");

    // LCU's WAMP-lite subscribe: [5, "OnJsonApiEvent"] subscribes to every
    // endpoint's change events; we filter to gameflow-phase on receipt.
    // `.into()` since tokio-tungstenite 0.26: `Message::Text` carries
    // `Utf8Bytes` rather than `String`. The conversion from an owned `String`
    // is not a copy — `Utf8Bytes` wraps `Bytes`, which takes ownership.
    ws.send(Message::Text(
        serde_json::to_string(&(5, "OnJsonApiEvent"))?.into(),
    ))
    .await
    .map_err(Box::new)?;

    // Separately from the connect above: reaching the socket and being
    // *subscribed* to it are two different claims, and a client that accepts
    // the connection and then refuses the subscribe would otherwise look
    // identical to one that works.
    info!("lcu", "subscribed to gameflow events; phases arrive on the socket");

    // Read the current phase *after* subscribing, never before: a change
    // landing between the two would then be delivered by the socket rather
    // than falling into the gap. The cost of that ordering is a possible
    // duplicate, which `last` below absorbs.
    let mut last: Option<GameflowPhase> = None;
    match http
        .get_json::<GameflowPhase>("/lol-gameflow/v1/gameflow-phase")
        .await
    {
        Ok(phase) => {
            last = Some(phase.clone());
            // `Polling` because that is literally what this was — an HTTP
            // read, not a socket frame.
            on_update(GameflowUpdate {
                phase,
                source: GameflowSource::Polling,
            });
        }
        // Not fatal: the socket is still live and will report the next
        // change. This only costs the current phase.
        Err(e) => warn!("lcu", "could not read the current gameflow phase: {e}"),
    }

    // Everything below is accounted for, because the socket's *ending* is the
    // thing this file could not previously describe (#146). An end-of-stream
    // falls out of the loop as `Ok(())`, which `watch` treats as nothing worth
    // mentioning before it sleeps and reconnects, so two of the three closes
    // in the alpha.49 log produced no line at all and the only evidence they
    // happened was the next "connected" line.
    let opened_at = std::time::Instant::now();
    let mut tally = FrameTally::default();

    let outcome = loop {
        let msg = match ws.next().await {
            // The stream ended. Either the peer's close frame has already been
            // counted by the arm below, or it hung up without one.
            None => break Ok(()),
            Some(Err(e)) => break Err(GameflowError::WebSocket(Box::new(e))),
            Some(Ok(msg)) => msg,
        };
        tally.count(&msg);

        if let Message::Text(text) = msg
            && let Some(update) = parse_gameflow_event(&text)
        {
            // De-duplicated like the polling path already does, so the
            // initial read and a change event carrying the same phase
            // do not both dispatch.
            if last.as_ref() == Some(&update.phase) {
                continue;
            }
            last = Some(update.phase.clone());
            on_update(update);
        }
    };

    // **At info, not debug, and once per connection.** The whole difficulty in
    // #146 was that the log could not answer "how long did that socket live
    // and why did it go away" without lining up timestamps by eye, and a line
    // that only appears when someone has already turned debug on cannot answer
    // it for a session that has already happened. One line every two or three
    // minutes is the same cadence as the two lines the connect already writes.
    //
    // It is logged on the error path too. `watch` warns and falls back to
    // polling there, and knowing the socket had run for two minutes and taken
    // thirty pings first is what separates "the client hung up" from "the
    // socket never worked".
    info!(
        "lcu",
        "the LCU event socket closed after {:.0}s: {tally}",
        opened_at.elapsed().as_secs_f64()
    );

    outcome
}

// --- Which game is running ------------------------------------------------

/// Which game the client says is in progress, flattened out of
/// `/lol-gameflow/v1/session` so nothing downstream carries the LCU's
/// nesting.
///
/// Read *during* the game rather than worked out afterwards. Deciding
/// which `gameId` just ended is the problem that kept `match_data` unwired
/// for months; the client will simply tell you while it is still running.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct GameIdentity {
    /// `None` when the client reports no game — the session exists in the
    /// lobby too, with a zeroed `gameId`.
    pub game_id: Option<i64>,
    /// Riot's real queue id, which is the only thing that can fill
    /// `recordings.queue`. The Live Client Data API never exposes one.
    pub queue_id: Option<i64>,
    /// Custom games never reach match history, so a post-game summary
    /// fetch for one would retry until it timed out and find nothing.
    pub is_custom: bool,
}

/// The slice of the session response we read. Every field is optional and
/// defaulted: this shape is taken from the LCU's own OpenAPI spec, not
/// from a response anyone here has seen, so a client that nests things
/// differently must degrade to "no id" rather than failing the request.
#[derive(Debug, Clone, Default, Deserialize)]
struct SessionDto {
    #[serde(rename = "gameData", default)]
    game_data: Option<GameDataDto>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct GameDataDto {
    #[serde(rename = "gameId", default)]
    game_id: Option<i64>,
    #[serde(rename = "isCustomGame", default)]
    is_custom_game: Option<bool>,
    #[serde(default)]
    queue: Option<QueueDto>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct QueueDto {
    #[serde(default)]
    id: Option<i64>,
}

/// Asks the client which game is running.
pub async fn fetch_session(http: &LcuHttpClient) -> Result<GameIdentity, LcuClientError> {
    let session: SessionDto = http.get_json("/lol-gameflow/v1/session").await?;
    Ok(identity(&session))
}

/// Pure half of `fetch_session`, so the shape can be tested against
/// fixture JSON without a client.
fn identity(session: &SessionDto) -> GameIdentity {
    let Some(game) = session.game_data.as_ref() else {
        return GameIdentity::default();
    };

    GameIdentity {
        // A session that is not in a game still has a `gameData` block,
        // with `gameId` zeroed. Zero is "no game", not game number zero.
        game_id: game.game_id.filter(|id| *id > 0),
        // Zero is *not* filtered here: it is the real queue id for a
        // custom game, and the library labels it "Custom". The LCU uses
        // negative values for "no queue".
        queue_id: game.queue.as_ref().and_then(|q| q.id).filter(|id| *id >= 0),
        is_custom: game.is_custom_game.unwrap_or(false),
    }
}

/// Parses one LCU WS event frame and extracts a gameflow phase update if
/// this frame is one. Pure and side-effect-free so it's testable against
/// fixture frames without a live socket.
fn parse_gameflow_event(text: &str) -> Option<GameflowUpdate> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let arr = value.as_array()?;
    if arr.len() < 3 || arr[1].as_str()? != "OnJsonApiEvent" {
        return None;
    }
    let event = &arr[2];
    if event.get("uri")?.as_str()? != "/lol-gameflow/v1/gameflow-phase" {
        return None;
    }
    let phase_str = event.get("data")?.as_str()?;
    Some(GameflowUpdate {
        phase: GameflowPhase::from(phase_str),
        source: GameflowSource::WebSocket,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
    use tokio_tungstenite::tungstenite::protocol::CloseFrame;

    // --- What one socket's life looked like (#146) ------------------------

    /// The line that was missing. Two of the three closes in the alpha.49 log
    /// produced no output at all, because the stream simply ended and
    /// `watch_via_websocket` returned `Ok(())`.
    #[test]
    fn a_stream_that_just_ends_says_so() {
        let tally = FrameTally::default();
        assert_eq!(
            tally.to_string(),
            "0 text, 0 binary, 0 ping, 0 pong; the stream ended without a close frame"
        );
    }

    /// **The answer to the question, when the client is willing to give one.**
    /// A close frame carries a code and a reason, and the read loop used to
    /// discard both: it matched `Message::Text` and let everything else fall
    /// through, so a deliberate hang-up and a socket that vanished read the
    /// same.
    #[test]
    fn a_close_frame_is_reported_with_its_code_and_reason() {
        let mut tally = FrameTally::default();
        tally.count(&Message::Close(Some(CloseFrame {
            code: CloseCode::Normal,
            reason: "idle timeout".into(),
        })));
        assert_eq!(
            tally.to_string(),
            "0 text, 0 binary, 0 ping, 0 pong; peer closed (code 1000, \"idle timeout\")"
        );
    }

    /// A close with no payload is legal and less informative, and the line has
    /// to be able to say which of the two happened.
    #[test]
    fn a_close_frame_with_no_payload_is_distinguishable() {
        let mut tally = FrameTally::default();
        tally.count(&Message::Close(None));
        assert!(tally.to_string().ends_with("peer closed (no code)"));
    }

    /// The counts that settle #146's hypothesis. It guessed the socket was
    /// being dropped because nothing ever writes after the subscribe, so a
    /// ping would go unanswered. Pings are counted here precisely so a real
    /// session can show whether any arrive; tungstenite queues the pong and
    /// flushes it from inside `read`, which `ws.next()` drives, so we do not
    /// send them and must not.
    #[test]
    fn frames_are_counted_by_kind() {
        let mut tally = FrameTally::default();
        tally.count(&Message::Text("{}".into()));
        tally.count(&Message::Text("{}".into()));
        tally.count(&Message::Ping(Vec::new().into()));
        tally.count(&Message::Pong(Vec::new().into()));
        tally.count(&Message::Binary(Vec::new().into()));

        assert_eq!(tally.text, 2);
        assert_eq!(tally.ping, 1);
        assert_eq!(tally.pong, 1);
        assert_eq!(tally.binary, 1);
        assert!(tally.to_string().starts_with("2 text, 1 binary, 1 ping, 1 pong;"));
    }

    /// The close frame is the last thing a well-behaved peer sends, so the
    /// counts that preceded it have to survive into the same line. A socket
    /// that took thirty pings and then closed normally is a very different
    /// report from one that closed having seen nothing.
    #[test]
    fn the_counts_survive_the_close() {
        let mut tally = FrameTally::default();
        for _ in 0..30 {
            tally.count(&Message::Ping(Vec::new().into()));
        }
        tally.count(&Message::Close(Some(CloseFrame {
            code: CloseCode::Away,
            reason: "".into(),
        })));
        assert_eq!(
            tally.to_string(),
            "0 text, 0 binary, 30 ping, 0 pong; peer closed (code 1001, \"\")"
        );
    }


    fn session(json: &str) -> SessionDto {
        serde_json::from_str(json).unwrap()
    }

    /// The shape the LCU's own OpenAPI spec describes for
    /// `LolGameflowGameflowSession`.
    #[test]
    fn reads_the_game_and_queue_ids_out_of_a_session() {
        let s = session(
            r#"{
                "phase": "InProgress",
                "gameData": {
                    "gameId": 5147823901,
                    "isCustomGame": false,
                    "queue": {"id": 420, "gameMode": "CLASSIC", "isRanked": true}
                }
            }"#,
        );

        assert_eq!(
            identity(&s),
            GameIdentity {
                game_id: Some(5147823901),
                queue_id: Some(420),
                is_custom: false,
            }
        );
    }

    /// The session exists in the lobby too, with `gameId` zeroed. Zero is
    /// "no game", not game number zero — recording it would attach every
    /// out-of-game recording to the same nonexistent match.
    #[test]
    fn a_zeroed_game_id_is_no_game_at_all() {
        let s = session(r#"{"gameData": {"gameId": 0, "queue": {"id": -1}}}"#);
        assert_eq!(identity(&s), GameIdentity::default());
    }

    /// Zero is a real queue id — it is what a custom game reports, and the
    /// library labels it "Custom". Only negatives mean "no queue".
    #[test]
    fn queue_zero_is_kept_because_it_means_custom() {
        let s = session(r#"{"gameData": {"gameId": 7, "isCustomGame": true, "queue": {"id": 0}}}"#);
        assert_eq!(
            identity(&s),
            GameIdentity {
                game_id: Some(7),
                queue_id: Some(0),
                is_custom: true,
            }
        );
    }

    /// This shape came from a spec, not from a response anyone here has
    /// seen. A client that nests it differently has to degrade to "no id",
    /// not fail the whole read.
    #[test]
    fn an_unrecognized_session_shape_yields_nothing_rather_than_erroring() {
        assert_eq!(identity(&session("{}")), GameIdentity::default());
        assert_eq!(
            identity(&session(r#"{"gameData": {"gameId": 9}}"#)),
            GameIdentity {
                game_id: Some(9),
                queue_id: None,
                is_custom: false,
            }
        );
    }

    #[test]
    fn phase_from_known_string() {
        assert_eq!(GameflowPhase::from("InProgress"), GameflowPhase::InProgress);
        assert_eq!(GameflowPhase::from("EndOfGame"), GameflowPhase::EndOfGame);
    }

    #[test]
    fn phase_from_unrecognized_string_is_unknown_not_an_error() {
        assert_eq!(
            GameflowPhase::from("SomeFuturePhase"),
            GameflowPhase::Unknown("SomeFuturePhase".to_string())
        );
    }

    #[test]
    fn parses_gameflow_event_frame() {
        let frame = r#"[8, "OnJsonApiEvent", {"data": "InProgress", "eventType": "Update", "uri": "/lol-gameflow/v1/gameflow-phase"}]"#;
        let update = parse_gameflow_event(frame).unwrap();
        assert_eq!(update.phase, GameflowPhase::InProgress);
        assert_eq!(update.source, GameflowSource::WebSocket);
    }

    #[test]
    fn ignores_events_for_other_endpoints() {
        let frame = r#"[8, "OnJsonApiEvent", {"data": {}, "eventType": "Update", "uri": "/lol-summoner/v1/current-summoner"}]"#;
        assert!(parse_gameflow_event(frame).is_none());
    }

    #[test]
    fn ignores_non_event_frames() {
        assert!(parse_gameflow_event(r#"[5, "OnJsonApiEvent"]"#).is_none());
        assert!(parse_gameflow_event("not json").is_none());
        assert!(parse_gameflow_event("{}").is_none());
    }
}
