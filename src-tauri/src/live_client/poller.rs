//! Polls the Live Client Data API while a game is running, with backoff
//! while the endpoint isn't reachable (no game, loading screen, or the
//! game just ended). DEVELOPMENT.md §3.2, issue acceptance: "poller with
//! backoff while port 2999 is down."

use crate::{debug, warn};
use super::client::LiveClientDataClient;
use super::events::AllGameData;
use std::time::Duration;

const MAX_BACKOFF: Duration = Duration::from_secs(10);

/// How many consecutive transport failures mean the game is really gone.
///
/// It used to be one, and one is wrong: `on_down` ends the recording and
/// tears down the poller, so a single dropped request cost the rest of the
/// game (#74 — a real game lost half an hour after 543 consecutive
/// successful polls). The trade is asymmetric. Being too tolerant costs a
/// few seconds of post-game screen on the end of a VOD; being too strict
/// costs the VOD.
const FAILURES_BEFORE_DOWN: u32 = 5;

/// Whether `failures` consecutive transport failures should end the
/// recording, given we had been receiving snapshots.
///
/// Pure so the threshold is pinned by a test rather than by playing five
/// games. See `FAILURES_BEFORE_DOWN`.
fn should_declare_down(was_up: bool, failures: u32) -> bool {
    was_up && failures >= FAILURES_BEFORE_DOWN
}

/// Polls forever, calling `on_snapshot` on every successful fetch and
/// `on_down` (at most once per transition into the down state) once the
/// endpoint has stopped responding for `FAILURES_BEFORE_DOWN` polls in a
/// row. Runs until the caller's task is aborted — the state machine owns
/// that lifecycle, starting this when gameflow enters
/// `InProgress`/`Reconnect` and stopping it once recording finalizes.
///
/// **Two kinds of failure, and only one of them ends a recording.** A
/// response we could not parse proves the game is running, so it is logged
/// and retried and never counts toward the threshold — see
/// `LiveClientError::means_endpoint_gone`. Only a request that got no
/// response at all counts.
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
    let mut failures: u32 = 0;

    loop {
        match client.fetch_all_game_data().await {
            Ok(snapshot) => {
                backoff = poll_interval;
                failures = 0;
                was_up = true;
                on_snapshot(snapshot);
                tokio::time::sleep(poll_interval).await;
            }
            Err(e) if !e.means_endpoint_gone() => {
                // Something answered. Whatever is wrong with it, the game
                // is alive, so this must never end a recording — it is the
                // one failure guaranteed to repeat, because a payload the
                // parser cannot read will not start reading next second.
                warn!("live-poll", "unreadable response, still recording: {e}");
                tokio::time::sleep(poll_interval).await;
            }
            Err(e) => {
                failures += 1;
                if should_declare_down(was_up, failures) {
                    warn!(
                        "live-poll",
                        "endpoint gone after {failures} consecutive failures, \
                         ending the recording: {e}"
                    );
                    on_down();
                    was_up = false;
                } else if was_up {
                    // Still hoping. Poll at the normal cadence rather than
                    // backing off: the backoff exists for the long stretch
                    // between games, and applying it here would stretch
                    // five failures across fifteen seconds instead of five.
                    warn!(
                        "live-poll",
                        "poll failed ({failures}/{FAILURES_BEFORE_DOWN}): {e}"
                    );
                    tokio::time::sleep(poll_interval).await;
                    continue;
                } else {
                    // Expected: the endpoint is not up until the game has
                    // loaded, and the poller starts before that.
                    debug!("live-poll", "not up yet: {e}");
                }
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The regression #74 is about: one failed poll used to end a
    /// recording outright.
    #[test]
    fn a_single_failure_never_ends_a_recording() {
        assert!(!should_declare_down(true, 1));
        assert!(!should_declare_down(true, 2));
    }

    #[test]
    fn enough_consecutive_failures_do() {
        assert!(should_declare_down(true, FAILURES_BEFORE_DOWN));
        assert!(should_declare_down(true, FAILURES_BEFORE_DOWN + 1));
    }

    /// Before the endpoint has ever answered there is no recording to end,
    /// and failures are the normal state — the poller starts while the
    /// game is still loading.
    #[test]
    fn failures_before_the_endpoint_ever_answered_are_not_a_transition() {
        assert!(!should_declare_down(false, FAILURES_BEFORE_DOWN * 10));
    }

    /// Five polls at the normal cadence, not five backoff steps. Applying
    /// the between-games backoff while still hoping would stretch the
    /// tolerance window to fifteen seconds.
    #[test]
    fn the_tolerance_window_is_about_five_seconds() {
        assert_eq!(FAILURES_BEFORE_DOWN, 5);
    }
}
