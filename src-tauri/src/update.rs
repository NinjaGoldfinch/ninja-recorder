//! In-app updates: the decision half.
//!
//! Windows builds install from an NSIS installer that CI publishes for every
//! commit on `main` ([docs/ci-and-releases.md](../../docs/ci-and-releases.md)),
//! and `tauri-plugin-updater` can fetch and run a newer one. The awkward part
//! is *when*.
//!
//! **The NSIS updater exits the app and runs the installer.** Doing that while
//! a game is being captured destroys the recording in flight — the one thing
//! this app exists not to do. So nothing here installs on its own: a check
//! produces an offer, the offer is only *installable* while the state machine
//! says nothing is being captured, and a person clicks the button. See
//! DEVELOPMENT.md §14 for why that is the trade rather than a silent
//! auto-update.
//!
//! This module is pure. `decide` reads no clock, opens no socket and touches
//! no `AppHandle`; the network half lives in `lib.rs`, which owns the plugin
//! and hands its result back through `core::Ctx`. That split is what makes the
//! gate — the part that can lose a VOD if it is wrong — directly unit-tested.

use crate::state_machine::GameState;

/// A newer version the endpoint offered, flattened out of the plugin's own
/// type so this module stays testable without one.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct UpdateOffer {
    pub version: String,
    /// The release notes, as `latest.json` carried them. Shown verbatim and
    /// therefore escaped by the caller — this is remote text.
    pub notes: Option<String>,
    pub pub_date: Option<String>,
}

/// What the About block renders, and the only thing the frontend ever sees.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum UpdateStatus {
    /// No updater in this build at all: a devtools bundle, a `tauri:dev` run,
    /// or a platform `latest.json` carries no entry for. Distinct from
    /// `UpToDate`, because "you are current" and "this build will never tell
    /// you" deserve different words on screen.
    Unsupported,
    /// Checked, and the endpoint had nothing newer.
    UpToDate,
    #[serde(rename_all = "camelCase")]
    Available {
        offer: UpdateOffer,
        /// Whether clicking Install right now is safe. Recomputed on every
        /// read rather than stored, so a status first built mid-game does not
        /// stay blocked after the game ends.
        installable: bool,
        /// Why not, when `installable` is false. Rendered next to the
        /// disabled button — a greyed-out control with no reason is the
        /// version of this that generates bug reports.
        blocked_reason: Option<String>,
    },
    /// The check itself failed: no network, a 404 from the endpoint, a
    /// signature that did not verify. Never installable.
    #[serde(rename_all = "camelCase")]
    Failed { error: String },
}

/// What the background task in `lib.rs` found, before the gate is applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckResult {
    /// The plugin is not registered in this build.
    Unsupported,
    NothingNewer,
    Found(UpdateOffer),
    Failed(String),
}

/// The two things the UI can ask the background half to do. One seam rather
/// than two closures, for the reason DEVELOPMENT.md §12's "One notifier, one
/// seam" gives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateRequest {
    Check,
    Install,
}

/// Whether it is safe to exit and hand control to an installer right now.
///
/// Two inputs and not one: `GameState` is the supervisor's view and
/// `is_recording` is the recorder's own, and they disagree for a moment
/// around a start or a finalize. Either saying yes is enough to refuse —
/// the cost of a needless refusal is one more click, and the cost of a
/// wrong permit is the game the user was in.
pub fn installable(state: &GameState, is_recording: bool) -> Result<(), String> {
    if is_recording {
        return Err("a recording is in progress".into());
    }
    match state {
        GameState::Idle | GameState::ClientRunning => Ok(()),
        // `WaitingForGame` is not recording yet, but the game is loading and
        // capture starts the moment Live Client Data answers. Restarting into
        // an installer here loses the game just as surely.
        GameState::WaitingForGame => Err("a game is about to start".into()),
        GameState::Recording => Err("a recording is in progress".into()),
        GameState::Finalizing => Err("a recording is still being finalized".into()),
    }
}

/// Turns a check result plus the app's live state into what the About block
/// shows. Pure — this is the tested half.
pub fn decide(found: &CheckResult, state: &GameState, is_recording: bool) -> UpdateStatus {
    match found {
        CheckResult::Unsupported => UpdateStatus::Unsupported,
        CheckResult::NothingNewer => UpdateStatus::UpToDate,
        CheckResult::Failed(e) => UpdateStatus::Failed { error: e.clone() },
        CheckResult::Found(offer) => match installable(state, is_recording) {
            Ok(()) => UpdateStatus::Available {
                offer: offer.clone(),
                installable: true,
                blocked_reason: None,
            },
            Err(why) => UpdateStatus::Available {
                offer: offer.clone(),
                installable: false,
                blocked_reason: Some(why),
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offer() -> UpdateOffer {
        UpdateOffer {
            version: "0.9.0".into(),
            notes: Some("- something".into()),
            pub_date: None,
        }
    }

    #[test]
    fn an_offer_while_idle_is_installable() {
        let status = decide(&CheckResult::Found(offer()), &GameState::Idle, false);
        assert_eq!(
            status,
            UpdateStatus::Available {
                offer: offer(),
                installable: true,
                blocked_reason: None,
            }
        );
    }

    #[test]
    fn a_running_client_is_not_by_itself_a_reason_to_refuse() {
        // The League client being open is the normal state of this app. Only
        // an actual game blocks.
        assert!(installable(&GameState::ClientRunning, false).is_ok());
    }

    /// The whole point of the gate. Each of these would cost the user the
    /// game they are in.
    #[test]
    fn nothing_installs_once_a_game_is_in_the_picture() {
        for state in [
            GameState::WaitingForGame,
            GameState::Recording,
            GameState::Finalizing,
        ] {
            let status = decide(&CheckResult::Found(offer()), &state, false);
            match status {
                UpdateStatus::Available {
                    installable,
                    blocked_reason,
                    ..
                } => {
                    assert!(!installable, "{state:?} should not be installable");
                    assert!(
                        blocked_reason.is_some(),
                        "{state:?} must say why it refused"
                    );
                }
                other => panic!("{state:?}: expected an offer, got {other:?}"),
            }
        }
    }

    /// The two views disagree for a moment around a start. The recorder's own
    /// answer has to be able to refuse on its own, or that window is a hole.
    #[test]
    fn the_recorder_can_refuse_even_when_the_state_machine_looks_idle() {
        assert!(installable(&GameState::Idle, true).is_err());
        let status = decide(&CheckResult::Found(offer()), &GameState::Idle, true);
        assert!(matches!(
            status,
            UpdateStatus::Available {
                installable: false,
                ..
            }
        ));
    }

    #[test]
    fn nothing_newer_is_up_to_date_and_a_missing_updater_is_not() {
        assert_eq!(
            decide(&CheckResult::NothingNewer, &GameState::Idle, false),
            UpdateStatus::UpToDate
        );
        assert_eq!(
            decide(&CheckResult::Unsupported, &GameState::Idle, false),
            UpdateStatus::Unsupported
        );
    }

    /// The wire shape `src/types.ts` mirrors by hand. Asserted rather than
    /// assumed: the tag and the camelCase rename are the two things that can
    /// change silently and would leave the About block rendering nothing at
    /// all, in a build nobody type-checks against Rust.
    #[test]
    fn the_wire_shape_is_what_the_frontend_mirrors() {
        let json = serde_json::to_value(decide(
            &CheckResult::Found(offer()),
            &GameState::Recording,
            false,
        ))
        .unwrap();
        assert_eq!(json["kind"], "available");
        assert_eq!(json["installable"], false);
        assert!(json["blockedReason"].is_string());
        assert_eq!(json["offer"]["version"], "0.9.0");
        // Not renamed: nested types keep their own attributes, exactly as
        // `RetentionPolicy` does (`core::dispatch`'s tests say the same).
        assert!(json["offer"].get("pub_date").is_some());

        assert_eq!(
            serde_json::to_value(UpdateStatus::UpToDate).unwrap()["kind"],
            "upToDate"
        );
        assert_eq!(
            serde_json::to_value(UpdateStatus::Unsupported).unwrap()["kind"],
            "unsupported"
        );
    }

    #[test]
    fn a_failed_check_is_never_installable() {
        let status = decide(
            &CheckResult::Failed("no network".into()),
            &GameState::Idle,
            false,
        );
        assert_eq!(
            status,
            UpdateStatus::Failed {
                error: "no network".into()
            }
        );
    }
}
