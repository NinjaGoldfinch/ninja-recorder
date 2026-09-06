//! Polls the Live Client Data API while a game is running, with backoff
//! while the endpoint isn't reachable (no game, loading screen, or the
//! game just ended). DEVELOPMENT.md §3.2, issue acceptance: "poller with
//! backoff while port 2999 is down."

use crate::{debug, warn};
use super::client::LiveClientDataClient;
use super::events::AllGameData;
use std::time::Duration;

const MAX_BACKOFF: Duration = Duration::from_secs(10);

/// Polls forever, calling `on_snapshot` on every successful fetch and
/// `on_down` (at most once per transition into the down state) whenever
/// the endpoint stops responding. Runs until the caller's task is
/// aborted — the state machine owns that lifecycle, starting
/// this when gameflow enters `InProgress`/`Reconnect` and stopping it once
/// recording finalizes.
pub async fn watch<OnSnapshot, OnDown>(
    client: &LiveClientDataClient,
    poll_interval: Duration,
    mut on_snapshot: OnSnapshot,
    mut on_down: OnDown,
) where
    OnSnapshot: FnMut(AllGameData) + Send,
    OnDown: FnMut() + Send,
{
    let mut backoff = poll_interval;
    let mut was_up = false;

    loop {
        match client.fetch_all_game_data().await {
            Ok(snapshot) => {
                backoff = poll_interval;
                was_up = true;
                on_snapshot(snapshot);
                tokio::time::sleep(poll_interval).await;
            }
            Err(e) => {
                if was_up {
                    // The transition that ends a recording (#74), so this
                    // is the one poll failure that must be visible at the
                    // default level. `LiveClientError`'s Display is what
                    // separates a transport failure from a payload that
                    // would not parse — the distinction #74's triage could
                    // not make, because this arm used to discard it.
                    warn!("live-poll", "endpoint stopped responding: {e}");
                    on_down();
                } else {
                    // Expected: the endpoint is not up until the game has
                    // loaded, and the poller starts before that.
                    debug!("live-poll", "not up yet: {e}");
                }
                was_up = false;
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }
        }
    }
}
