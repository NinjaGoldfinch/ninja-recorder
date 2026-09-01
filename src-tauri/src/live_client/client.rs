//! HTTP client for the Live Client Data API. Unlike the LCU API this one
//! needs no auth, but it's still self-signed TLS on localhost and only up
//! while a game is actually running. DEVELOPMENT.md §3.2.

use super::events::AllGameData;
use crate::fixtures;

const BASE_URL: &str = "https://127.0.0.1:2999";
const ALL_GAME_DATA_PATH: &str = "/liveclientdata/allgamedata";

#[derive(Debug, thiserror::Error)]
pub enum LiveClientError {
    #[error("request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("failed to build http client: {0}")]
    Build(reqwest::Error),
    #[error("failed to parse response json: {0}")]
    Parse(#[from] serde_json::Error),
}

pub struct LiveClientDataClient {
    client: reqwest::Client,
}

impl LiveClientDataClient {
    pub fn new() -> Result<Self, LiveClientError> {
        let client = reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
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
}

impl Default for LiveClientDataClient {
    fn default() -> Self {
        Self::new().expect("failed to build reqwest client")
    }
}
