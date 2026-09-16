//! Checking for updates, from the process that can refuse one. WS3 task 3.6.
//!
//! `tauri-plugin-updater` did this in the UI, and it needs an `AppHandle`, so
//! the daemon cannot use it. It also should not: the question "may this install
//! run now" is answered by whether a game is being recorded, and the daemon is
//! the process that knows.
//!
//! ## The check half only
//!
//! This fetches the manifest, decides what it means, records it and publishes
//! it. It does not download or install anything. The install half is the rest
//! of WS3.6: `reqwest` to fetch the installer, `minisign-verify` against the
//! baked public key, then run it with `/S` once nothing is recording.
//!
//! Until that lands, `install_update` refuses. It refused before this too, and
//! for a worse reason: since WS3.4 every command runs in the daemon, whose
//! update seam nothing had ever filled in, so an install attempt reached a
//! `None` and said "not available in this build" on a build where it was.
//!
//! ## What the frontend sees
//!
//! Both the stored result and an event. `get_update_status` answers from the
//! `Ctx` cell, which is what the About block reads on demand; `Event::UpdateStatus`
//! is pushed so a window open at the time does not have to poll. They carry the
//! same value by construction, because this is the only thing that sets it.

use std::sync::Arc;
use std::time::Duration;

use crate::contract::events::Event;
use crate::core::Ctx;
use crate::daemon::snapshot::Stream;
use crate::update::{self, CheckResult};
use crate::{info, warn};

/// How long after startup the first check runs.
///
/// Late enough that it is never competing with the recorder backend coming up,
/// the database opening or the pipe being bound, none of which should wait on a
/// network round trip to GitHub.
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(30);

/// And how often after that.
///
/// Deliberately slack: CI publishes a release for every commit that lands on
/// `main`, so "something newer exists" is true most days, and a tighter loop
/// would only rediscover the same answer.
const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

/// How long to wait on the endpoint before giving up.
///
/// A check that hangs is worse than one that fails: the status stays `Checking`
/// forever and the About block shows a spinner with nothing behind it.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// Starts the periodic check, for the life of the daemon.
///
/// Returns immediately. Every outcome is recorded rather than returned: a
/// failed check is a *state* the About block renders, not an error to
/// propagate. Someone's network being down is not a bug.
pub fn spawn_checks(ctx: Arc<Ctx>, events: Stream) {
    if !crate::updates_enabled() {
        // A devtools build that updated itself would replace itself with the
        // production app (DEVELOPMENT.md §14). Said once, and recorded as a
        // state rather than left looking like a check that never answered.
        info!("update", "devtools build: updates are off");
        publish(&ctx, &events, CheckResult::Unsupported);
        return;
    }

    tokio::spawn(async move {
        tokio::time::sleep(FIRST_CHECK_DELAY).await;
        loop {
            let found = check(&ctx).await;
            publish(&ctx, &events, found);
            tokio::time::sleep(CHECK_INTERVAL).await;
        }
    });
}

/// One check: fetch the channel's manifest and decide what it means.
pub async fn check(ctx: &Arc<Ctx>) -> CheckResult {
    let endpoint = endpoint_for_channel(ctx);

    let client = match reqwest::Client::builder().timeout(REQUEST_TIMEOUT).build() {
        Ok(client) => client,
        Err(e) => return CheckResult::Failed(format!("Could not check for updates: {e}")),
    };

    let response = match client.get(&endpoint).send().await {
        Ok(response) => response,
        Err(e) => return CheckResult::Failed(format!("Could not check for updates: {e}")),
    };
    if !response.status().is_success() {
        // A 404 on the alpha endpoint is the ordinary state of a repository
        // that has not published one yet, and it is not worth alarming anybody
        // about. Reported as a failure rather than as "nothing newer" all the
        // same: the two are different, and a silently missing endpoint is how
        // an app stops updating without anyone noticing.
        return CheckResult::Failed(format!(
            "Could not check for updates: the update server answered {}",
            response.status()
        ));
    }

    match response.text().await {
        Ok(body) => update::evaluate(&body, update::current_version()),
        Err(e) => CheckResult::Failed(format!("Could not check for updates: {e}")),
    }
}

/// Which manifest to read, from the user's channel preference.
///
/// A preference read that fails falls back to stable rather than propagating:
/// the conservative channel is the right answer to "we could not tell", and the
/// alternative is an install that stops checking because its settings table
/// hiccuped.
fn endpoint_for_channel(ctx: &Arc<Ctx>) -> String {
    let stored = ctx
        .db
        .get_ui_prefs()
        .ok()
        .and_then(|prefs| prefs.get(update::CHANNEL_PREF_KEY).cloned());

    match update::Channel::from_pref(stored.as_deref()) {
        update::Channel::Alpha => update::ALPHA_ENDPOINT.to_string(),
        update::Channel::Stable => update::STABLE_ENDPOINT.to_string(),
    }
}

/// Records what a check found, and tells anyone listening.
///
/// The two go together every time: a stored result nothing is told about is a
/// status the About block shows six hours late.
fn publish(ctx: &Arc<Ctx>, events: &Stream, found: CheckResult) {
    match &found {
        CheckResult::Found(offer) => info!("update", "update available: {}", offer.version),
        CheckResult::Failed(why) => warn!("update", "{why}"),
        _ => {}
    }
    ctx.set_update_check_result(found);

    // Rendered through the same function the command uses, so the event and a
    // later `get_update_status` cannot disagree about what is installable: that
    // answer depends on whether a game is running, which changes without
    // anything here being told.
    match crate::core::get_update_status(ctx) {
        Ok(status) => events.publish(Event::UpdateStatus { status }),
        Err(e) => warn!("update", "could not render the update status: {e}"),
    }
}
