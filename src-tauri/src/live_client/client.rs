//! HTTP client for the Live Client Data API. Unlike the LCU API this one
//! needs no auth, but it's still self-signed TLS on localhost and only up
//! while a game is actually running. DEVELOPMENT.md §3.2.

use super::events::AllGameData;
use crate::fixtures;
use std::time::Duration;

const BASE_URL: &str = "https://127.0.0.1:2999";
const ALL_GAME_DATA_PATH: &str = "/liveclientdata/allgamedata";

/// The endpoint is on loopback and answers in about ten milliseconds, so
/// anything approaching this is a hang rather than a slow reply.
///
/// reqwest applies **no** timeout by default, which meant a stalled
/// request blocked the poll loop indefinitely: markers and samples stopped
/// silently while the recording carried on, and nothing ever declared the
/// endpoint down because no error was ever returned. A bounded wait turns
/// that into an ordinary failure the poller can reason about (#74).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, thiserror::Error)]
pub enum LiveClientError {
    #[error("request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("failed to build http client: {0}")]
    Build(reqwest::Error),
    #[error("failed to parse response json: {0}")]
    Parse(#[from] serde_json::Error),
}

impl LiveClientError {
    /// Whether this failure means **the game is gone**, as opposed to the
    /// endpoint being present and unhappy.
    ///
    /// The difference decides whether a recording ends, so it is not a
    /// detail. A payload we could not read is proof the game is *running*
    /// — something answered — and treating that as "game over" is what
    /// ended a recording nine minutes into a game in #74. Same for an HTTP
    /// error status: a 500 came from a live server.
    ///
    /// Only a request that never got a response at all — connection
    /// refused, or the timeout above — means the process behind port 2999
    /// has gone.
    pub fn means_endpoint_gone(&self) -> bool {
        match self {
            // `status()` is `None` when no response was received.
            LiveClientError::Request(e) => e.status().is_none(),
            // Something answered; it just was not what we could read.
            LiveClientError::Parse(_) => false,
            // Building the client failed, which happens once at startup
            // and says nothing about the game.
            LiveClientError::Build(_) => false,
        }
    }
}

pub struct LiveClientDataClient {
    client: reqwest::Client,
}

impl LiveClientDataClient {
    pub fn new() -> Result<Self, LiveClientError> {
        let client = reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(LiveClientError::Build)?;
        Ok(Self { client })
    }

    /// Fetches the full game data snapshot. Errors whenever the endpoint
    /// isn't reachable — no game running, loading screen not finished yet,
    /// or the game just ended — which is the expected steady state most of
    /// the time, not exceptional; callers (the poller) treat it as "not up
    /// right now" rather than a hard failure.
    pub async fn fetch_all_game_data(&self) -> Result<AllGameData, LiveClientError> {
        let text = self
            .client
            .get(format!("{BASE_URL}{ALL_GAME_DATA_PATH}"))
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;

        fixtures::record("live-client", ALL_GAME_DATA_PATH, &text);

        Ok(serde_json::from_str(&text)?)
    }

    /// The same request, returned unparsed. The dev portal wants the raw
    /// payload — to save as a fixture, and to see fields `AllGameData`
    /// deliberately drops — which a typed fetch can't give it.
    #[cfg(feature = "devtools")]
    pub async fn fetch_all_game_data_raw(&self) -> Result<serde_json::Value, LiveClientError> {
        let text = self
            .client
            .get(format!("{BASE_URL}{ALL_GAME_DATA_PATH}"))
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;

        fixtures::record("live-client", ALL_GAME_DATA_PATH, &text);

        Ok(serde_json::from_str(&text)?)
    }
}

impl Default for LiveClientDataClient {
    fn default() -> Self {
        Self::new().expect("failed to build reqwest client")
    }
}
