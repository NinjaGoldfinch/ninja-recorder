//! Polls the Live Client Data API while a game is running, with backoff
//! while the endpoint isn't reachable (no game, loading screen, or the
//! game just ended). DEVELOPMENT.md §3.2, issue acceptance: "poller with
//! backoff while port 2999 is down."

use crate::{debug, warn};
use super::client::{LiveClientDataClient, LiveClientError};
use super::events::AllGameData;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Where the first unreadable response of a game is kept, in the logs
/// directory, overwritten by the next game that has one (#305).
///
/// The one thing the #305 report could not say was *what* the response
/// looked like: fixture capture is off by default, and the log line gave a
/// line and column in a payload that was gone. A person reading the log
/// after the fact now has the payload beside it.
const UNREADABLE_FILE: &str = "live-client-unreadable.json";

/// How much of an unreadable response is kept. A real `allgamedata` is tens
/// of kilobytes late in a game, so this keeps any plausible one whole while
/// making sure a runaway response cannot fill the disk from a poll loop.
const UNREADABLE_CAP_BYTES: usize = 1024 * 1024;

/// Writes `raw` to `dir/UNREADABLE_FILE`, cut at `UNREADABLE_CAP_BYTES`.
fn save_unreadable(dir: &Path, raw: &str) -> std::io::Result<PathBuf> {
    let path = dir.join(UNREADABLE_FILE);
    let bytes = raw.as_bytes();
    std::fs::write(&path, &bytes[..bytes.len().min(UNREADABLE_CAP_BYTES)])?;
    Ok(path)
}

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
///
/// **An unreadable response is still reported, through `on_unreadable`**
/// (#305). It does not end anything, but it means the stretch that follows
/// the last good snapshot is *game*, not post-game, and the trim must not
/// take it for the end screen. A 404 is not reported: that is the API
/// saying the game is over, which is the end signal the trim wants.
///
/// One call to `watch` is one game — the supervisor starts it per game — so
/// "the first unreadable response of the game" is the first one this call
/// sees. That one is saved beside the logs (`UNREADABLE_FILE`).
pub async fn watch<OnSnapshot, OnUnreadable, OnDown>(
    client: &LiveClientDataClient,
    poll_interval: Duration,
    mut on_snapshot: OnSnapshot,
    mut on_unreadable: OnUnreadable,
    mut on_down: OnDown,
) where
    OnSnapshot: FnMut(AllGameData) + Send,
    OnUnreadable: FnMut() + Send,
    OnDown: FnMut() + Send,
{
    let mut backoff = poll_interval;
    let mut was_up = false;
    let mut failures: u32 = 0;
    let mut saved_unreadable = false;

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
                //
                // **A 404 is not "whatever is wrong with it".** It is this
                // API answering correctly that no game is in progress, which
                // happens for a few seconds at the end of every game while the
                // server is still up. Treated as an unreadable response it
                // produced three or four warnings per game, for an expected
                // answer, in the log a person reads when something has gone
                // wrong. The *behaviour* is unchanged and deliberately so:
                // ending a recording on a 404 would cut it short here and cut
                // it before it began during a loading screen.
                if e.means_no_game() {
                    debug!("live-poll", "the endpoint says no game in progress; still recording");
                } else {
                    warn!("live-poll", "unreadable response, still recording: {e}");
                    if let LiveClientError::Unreadable { raw, .. } = &e
                        && !saved_unreadable
                    {
                        saved_unreadable = true;
                        match crate::log::dir().map(|dir| save_unreadable(&dir, raw)) {
                            Some(Ok(path)) => warn!(
                                "live-poll",
                                "saved the first unreadable response of this game to {}",
                                path.display()
                            ),
                            Some(Err(err)) => warn!(
                                "live-poll",
                                "could not save the unreadable response: {err}"
                            ),
                            None => debug!("live-poll", "no log directory to save the response in"),
                        }
                    }
                    on_unreadable();
                }
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

    /// The saved response is the response, and a runaway one is cut rather
    /// than written whole from a loop that runs every second.
    #[test]
    fn an_unreadable_response_is_saved_whole_up_to_the_cap() {
        let dir = std::env::temp_dir().join(format!("nr-unreadable-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let path = save_unreadable(&dir, r#"{"allPlayers":[{"items":{}}]}"#).unwrap();
        assert_eq!(path, dir.join(UNREADABLE_FILE));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), r#"{"allPlayers":[{"items":{}}]}"#);

        // Overwritten, not appended: one file per game, not per poll.
        let big = "x".repeat(UNREADABLE_CAP_BYTES + 10);
        save_unreadable(&dir, &big).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().len() as usize, UNREADABLE_CAP_BYTES);

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
