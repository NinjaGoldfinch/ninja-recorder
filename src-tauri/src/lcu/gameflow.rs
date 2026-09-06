//! Gameflow phase tracking: LCU WebSocket subscription with a polling
//! fallback if the socket can't be established. DEVELOPMENT.md §3.1, §3.4.
//!
//! `watch` is driven continuously by the state machine's supervisor
//! (`state_machine::supervisor`), which owns the lockfile-change lifecycle
//! (when to start/stop/restart it). Not verified against a real LCU
//! connection yet — no League client is installed on the machine this was
//! written on (DEVELOPMENT.md §9).

use crate::warn;
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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
        if let Err(e) = watch_via_websocket(lockfile, &mut on_update).await {
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

async fn watch_via_websocket<F>(
    lockfile: &LockfileInfo,
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

    // LCU's WAMP-lite subscribe: [5, "OnJsonApiEvent"] subscribes to every
    // endpoint's change events; we filter to gameflow-phase on receipt.
    ws.send(Message::Text(serde_json::to_string(&(
        5,
        "OnJsonApiEvent",
    ))?))
    .await
    .map_err(Box::new)?;

    while let Some(msg) = ws.next().await {
        if let Message::Text(text) = msg.map_err(Box::new)? {
            if let Some(update) = parse_gameflow_event(&text) {
                on_update(update);
            }
        }
    }

    Ok(())
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
