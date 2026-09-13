//! Thin authenticated HTTP client for the LCU API.
//!
//! The LCU API is self-signed TLS on localhost — accepting invalid certs
//! is scoped to this client's own `reqwest::Client` instance only via
//! `danger_accept_invalid_certs`, never anything global. See
//! DEVELOPMENT.md §3.1.

use super::lockfile::LockfileInfo;
use crate::fixtures;
use serde::de::DeserializeOwned;

#[derive(Debug, thiserror::Error)]
pub enum LcuClientError {
    #[error("request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("failed to build http client: {0}")]
    Build(reqwest::Error),
    #[error("failed to parse response json: {0}")]
    Parse(#[from] serde_json::Error),
}

pub struct LcuHttpClient {
    client: reqwest::Client,
    base_url: String,
    password: String,
}

impl LcuHttpClient {
    pub fn new(lockfile: &LockfileInfo) -> Result<Self, LcuClientError> {
        let client = reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .build()
            .map_err(LcuClientError::Build)?;

        Ok(Self {
            client,
            base_url: lockfile.base_url(),
            password: lockfile.password.clone(),
        })
    }

    /// GETs `path` and deserializes the JSON body as `T`. When fixture
    /// recording is enabled (`NINJA_RECORDER_RECORD_FIXTURES`), the raw
    /// response is also written to `fixtures/lcu/` before parsing —
    /// DEVELOPMENT.md §3.3.
    pub async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T, LcuClientError> {
        let url = format!("{}{}", self.base_url, path);
        let text = self
            .client
            .get(url)
            .basic_auth("riot", Some(&self.password))
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;

        fixtures::record("lcu", path, &text);

        Ok(serde_json::from_str(&text)?)
    }
}

/// `Authorization: Basic ...` header value for the given lockfile password.
/// Shared with the WebSocket handshake in `gameflow`, which can't use
/// reqwest's `basic_auth` helper.
pub fn basic_auth_header(password: &str) -> String {
    use base64::Engine;
    let token = base64::engine::general_purpose::STANDARD.encode(format!("riot:{}", password));
    format!("Basic {}", token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_auth_header_is_base64_of_riot_colon_password() {
        // "riot:hunter2" base64-encoded, verified against a known-good encoder.
        assert_eq!(
            basic_auth_header("hunter2"),
            "Basic cmlvdDpodW50ZXIy"
        );
    }
}
