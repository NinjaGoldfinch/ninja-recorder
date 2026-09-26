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
    /// A response `AllGameData` could not read, with where it failed and what
    /// it was (#305).
    ///
    /// `path` is the field, as `serde_path_to_error` spells it
    /// (`allPlayers[0].items`), because serde_json on its own reports a line
    /// and column in a response that is gone by the time anyone reads the
    /// log. `raw` is the response itself, so the poller can keep the first
    /// one of a game (`poller::save_unreadable`).
    #[error("failed to parse response json at {path}: {source}")]
    Unreadable {
        path: String,
        source: serde_json::Error,
        raw: String,
    },
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
    /// Whether the endpoint answered **404**, which from this API means
    /// there is no game in progress.
    ///
    /// Deliberately *not* folded into `means_endpoint_gone`. The two questions
    /// look the same and are not: that one decides whether a recording ends,
    /// and a 404 is a bad reason to end one. It appears while a game is
    /// loading, before the endpoint has anything to serve, and again for a few
    /// seconds after the game ends while the server is still up. Ending on the
    /// first would cut a recording before it started and ending on the second
    /// would beat `trim` to work it already does.
    ///
    /// What it is for is the log. A 404 here is the API answering correctly,
    /// and `watch` warned about it three or four times a game as an
    /// "unreadable response", which is how a warning stops meaning anything.
    pub fn means_no_game(&self) -> bool {
        match self {
            LiveClientError::Request(e) => e.status() == Some(reqwest::StatusCode::NOT_FOUND),
            LiveClientError::Parse(_)
            | LiveClientError::Unreadable { .. }
            | LiveClientError::Build(_) => false,
        }
    }

    pub fn means_endpoint_gone(&self) -> bool {
        match self {
            // `status()` is `None` when no response was received.
            LiveClientError::Request(e) => e.status().is_none(),
            // Something answered; it just was not what we could read.
            LiveClientError::Parse(_) | LiveClientError::Unreadable { .. } => false,
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

        parse_all_game_data(text)
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

/// Parses one `allgamedata` response, naming the JSON path of a failure.
///
/// Takes the text by value because the failure path keeps it: the poller
/// saves the first unreadable response of a game next to the logs.
pub fn parse_all_game_data(text: String) -> Result<AllGameData, LiveClientError> {
    let mut json = serde_json::Deserializer::from_str(&text);
    let (path, source) = match serde_path_to_error::deserialize(&mut json) {
        // `end` rejects trailing characters, which `serde_json::from_str`
        // did too.
        Ok(snapshot) => match json.end() {
            Ok(()) => return Ok(snapshot),
            Err(source) => ("(after the document)".to_string(), source),
        },
        Err(e) => (e.path().to_string(), e.into_inner()),
    };
    Err(LiveClientError::Unreadable { path, source, raw: text })
}

impl Default for LiveClientDataClient {
    fn default() -> Self {
        Self::new().expect("failed to build reqwest client")
    }
}

#[cfg(test)]
mod tests {
    //! The two questions a failed poll is asked, and why they are different.
    //!
    //! A real `reqwest::Error` is the only way to test these: the variants
    //! wrap one and it cannot be built by hand. So the tests make an actual
    //! request to a listener of their own and classify what comes back, which
    //! also means they are testing `reqwest`'s behaviour rather than an
    //! assumption about it.

    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Answers one request with the given status line, and returns its URL.
    ///
    /// Hand-rolled for the reason `daemon::update`'s tests give: this is four
    /// lines of HTTP, and the alternative is a dependency in the shipped tree
    /// for the benefit of one test module.
    async fn answering(status: &'static str) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut scratch = [0u8; 1024];
            let _ = socket.read(&mut scratch).await;
            let response =
                format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            let _ = socket.write_all(response.as_bytes()).await;
        });
        format!("http://{addr}/liveclientdata/allgamedata")
    }

    async fn error_from(status: &'static str) -> LiveClientError {
        let url = answering(status).await;
        let failed = reqwest::get(&url).await.unwrap().error_for_status().unwrap_err();
        LiveClientError::Request(failed)
    }

    /// The classification this exists for. A 404 is the API saying there is no
    /// game, which is an expected answer at both ends of one.
    #[tokio::test]
    async fn a_404_means_no_game() {
        let e = error_from("404 Not Found").await;
        assert!(e.means_no_game());
    }

    /// **And it must still not end a recording.** These two are asked
    /// separately because they have different answers for the same error, and
    /// collapsing them would cut a recording off at the moment the game ends,
    /// or before it begins while the client is still loading.
    #[tokio::test]
    async fn a_404_does_not_mean_the_endpoint_is_gone() {
        let e = error_from("404 Not Found").await;
        assert!(!e.means_endpoint_gone(), "a 404 must never end a recording");
    }

    /// A live server having a bad day is not a game that ended, and is worth
    /// a warning rather than a debug line.
    #[tokio::test]
    async fn a_500_is_neither() {
        let e = error_from("500 Internal Server Error").await;
        assert!(!e.means_no_game(), "a 500 says nothing about whether a game is running");
        assert!(!e.means_endpoint_gone(), "something answered, so the game is alive");
    }

    /// A response that still fails names the field it failed at, and keeps
    /// itself for the poller to save (#305). The log line used to say "line
    /// 237 column 12" of a response nobody had.
    #[test]
    fn an_unreadable_response_names_its_path_and_keeps_its_text() {
        let text = r#"{"gameData": {"gameTime": "late"}}"#.to_string();
        let e = parse_all_game_data(text.clone()).unwrap_err();
        match &e {
            LiveClientError::Unreadable { path, raw, .. } => {
                assert_eq!(path, "gameData.gameTime");
                assert_eq!(raw, &text);
            }
            other => panic!("expected Unreadable, got {other:?}"),
        }
        assert!(e.to_string().contains("at gameData.gameTime"), "{e}");
        // Something answered, so the game is alive.
        assert!(!e.means_endpoint_gone());
        assert!(!e.means_no_game());
    }

    /// And a readable one still reads, trailing whitespace and all.
    #[test]
    fn a_readable_response_parses() {
        let snapshot = parse_all_game_data("{\"gameData\": {\"gameTime\": 1.5}}\n".into()).unwrap();
        assert_eq!(snapshot.game_data.game_time, 1.5);
    }

    /// Nothing answered, which is the one case that does end a recording.
    #[tokio::test]
    async fn a_refused_connection_means_the_endpoint_is_gone() {
        // Bound and dropped, so the port is closed and nothing is listening.
        let port = {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            listener.local_addr().unwrap().port()
        };
        let failed = reqwest::get(format!("http://127.0.0.1:{port}/")).await.unwrap_err();
        let e = LiveClientError::Request(failed);

        assert!(e.means_endpoint_gone());
        assert!(!e.means_no_game(), "no response is not a 404");
    }
}
