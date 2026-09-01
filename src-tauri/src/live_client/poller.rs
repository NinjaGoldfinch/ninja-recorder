//! Polls the Live Client Data API while a game is running, with backoff
//! while the endpoint isn't reachable (no game, loading screen, or the
//! game just ended). DEVELOPMENT.md §3.2, issue acceptance: "poller with
//! backoff while port 2999 is down."

use super::client::LiveClientDataClient;
use super::events::AllGameData;
use std::time::Duration;

const MAX_BACKOFF: Duration = Duration::from_secs(10);

/// Polls forever, calling `on_snapshot` on every successful fetch and
/// `on_down` (at most once per transition into the down state) whenever
/// the endpoint stops responding. Runs until the caller's task is
/// aborted — the state machine (Phase 3) owns that lifecycle, starting
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
            Err(_) => {
                if was_up {
                    on_down();
                }
                was_up = false;
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }
        }
    }
}
