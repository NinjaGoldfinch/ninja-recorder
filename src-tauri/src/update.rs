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

/// Which stream of releases this install follows.
///
/// **The channels are separated by endpoint, never by comparison**, and that
/// is not a stylistic choice. The updater compares with plain semver `>`, and
/// semver says `1.1.0-alpha.1 > 1.0.0` is *true* — so a stable install that
/// could see the alpha manifest at all would be offered alphas by default.
/// Two manifests on two URLs is the only arrangement that holds
/// (DEVELOPMENT.md §15).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Channel {
    Stable,
    Alpha,
}

/// `settings_kv` key holding the channel. A missing key means stable, which
/// is how every pref in that table works — adding one needs no migration.
pub const CHANNEL_PREF_KEY: &str = "updateChannel";

/// The alpha manifest's URL.
///
/// Only the *alpha* endpoint lives here. Stable's is in `tauri.conf.json`
/// under `plugins.updater.endpoints` and is used as configured, so there is
/// one copy of it rather than two that can drift.
///
/// This URL is a permanent prerelease whose single asset is replaced on every
/// build — see `ci.yml`'s "Publish the alpha manifest" step. It cannot be
/// `/releases/latest/download/`, because GitHub excludes prereleases from
/// `latest`, which is exactly what keeps the stable channel clean.
/// Where the stable channel's manifest lives.
///
/// **A second copy of a URL that is also in `tauri.conf.json`**, under
/// `plugins.updater.endpoints`, and that was worth avoiding until the daemon
/// needed one: it builds no Tauri app, so it cannot read the plugin's config.
///
/// The copy is pinned rather than trusted. `the_stable_endpoint_matches_tauri_conf`
/// reads the config back and fails if the two ever differ, the same way
/// `daemon::IDENTIFIER` is pinned, and for the same reason: a mismatch would
/// not crash anything. It would quietly check the wrong place forever.
pub const STABLE_ENDPOINT: &str =
    "https://github.com/NinjaGoldfinch/ninja-recorder-v2/releases/latest/download/latest.json";

pub const ALPHA_ENDPOINT: &str =
    "https://github.com/NinjaGoldfinch/ninja-recorder-v2/releases/download/alpha/alpha.json";

impl Channel {
    /// Reads the stored pref. Anything unrecognised is **stable**, not an
    /// error: a corrupt or hand-edited value should leave an install on the
    /// conservative channel rather than silently opting it into prereleases.
    pub fn from_pref(value: Option<&str>) -> Self {
        match value {
            Some("alpha") => Self::Alpha,
            _ => Self::Stable,
        }
    }
}

/// A newer version the endpoint offered, flattened out of the plugin's own
/// type so this module stays testable without one.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
pub struct UpdateOffer {
    pub version: String,
    /// The release notes, as `latest.json` carried them. Shown verbatim and
    /// therefore escaped by the caller — this is remote text.
    pub notes: Option<String>,
    pub pub_date: Option<String>,
}

/// What the About block renders, and the only thing the frontend ever sees.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum UpdateStatus {
    /// No updater in this build at all: a devtools bundle, a `tauri:dev` run,
    /// or a platform `latest.json` carries no entry for. Distinct from
    /// `UpToDate`, because "you are current" and "this build will never tell
    /// you" deserve different words on screen.
    Unsupported,
    /// Nothing has been checked yet.
    ///
    /// **Deliberately not `Unsupported`.** This used to share that variant,
    /// which meant a perfectly ordinary production build said "Updates are
    /// not available in this build" for the first thirty seconds after
    /// launch — the most alarming possible wording for "hang on".
    Checking,
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
    /// The plugin is not registered in this build. Set explicitly by
    /// `wire_updates` when it declines to wire anything, never as a default.
    Unsupported,
    /// The starting state: wired, but the first check has not answered yet.
    Pending,
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
        CheckResult::Pending => UpdateStatus::Checking,
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

    /// Anything but an exact "alpha" leaves the install on stable. A
    /// hand-edited or corrupted pref must not opt someone into prereleases.
    #[test]
    fn only_an_exact_alpha_pref_selects_the_alpha_channel() {
        assert_eq!(Channel::from_pref(Some("alpha")), Channel::Alpha);
        for value in [None, Some(""), Some("stable"), Some("Alpha"), Some("beta")] {
            assert_eq!(Channel::from_pref(value), Channel::Stable, "{value:?}");
        }
    }

    #[test]
    fn the_alpha_endpoint_is_a_url() {
        let url = ALPHA_ENDPOINT.parse::<tauri::Url>();
        assert!(url.is_ok(), "{ALPHA_ENDPOINT} should parse");
        // Not `/releases/latest/`: GitHub excludes prereleases from `latest`,
        // so that path can never serve an alpha.
        assert!(!ALPHA_ENDPOINT.contains("/releases/latest/"));
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

    /// The bug this variant exists for: a production build reported
    /// "not available in this build" until its first check landed.
    #[test]
    fn not_having_checked_yet_is_not_the_same_as_not_being_able_to() {
        assert_eq!(
            decide(&CheckResult::Pending, &GameState::Idle, false),
            UpdateStatus::Checking
        );
        assert_ne!(
            decide(&CheckResult::Pending, &GameState::Idle, false),
            UpdateStatus::Unsupported
        );
        assert_eq!(
            serde_json::to_value(UpdateStatus::Checking).unwrap()["kind"],
            "checking"
        );
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

// --- The manifest an endpoint serves ---------------------------------------
//
// WS3.6. `tauri-plugin-updater` fetched and parsed this, and it needs an
// `AppHandle`, so the daemon cannot use it. What the daemon needs instead is
// the two decisions the plugin was making on our behalf: is this document
// well-formed, and is the version in it newer than ours.
//
// Both are pure, so both are tested here rather than against a network.

/// What an update endpoint serves, as much of it as matters.
///
/// The shape is Tauri's, because the files are already published in it and a
/// v2.0 client has to keep reading what v1 wrote. Unknown fields are ignored
/// rather than refused: the publisher is us, but a future field added for a
/// newer client must not stop an older one checking.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct Manifest {
    pub version: String,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub pub_date: Option<String>,
    pub platforms: std::collections::HashMap<String, ManifestPlatform>,
}

/// One platform's entry: where to get it, and what proves it is ours.
///
/// Neither field is read yet, and both are declared now on purpose. They are
/// what the *install* half of WS3.6 needs — the URL to fetch and the signature
/// to check it against — and a manifest type that described only the half
/// already in use would have to be widened by whoever writes that, at which
/// point the shape stops being a statement about the document and becomes a
/// record of what happened to be needed. Clippy runs without `--all-targets`,
/// so `-D warnings` would fail on them meanwhile.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ManifestPlatform {
    /// The minisign signature over the file at `url`, base64 as minisign
    /// writes it. Checked at install time, not at check time.
    pub signature: String,
    pub url: String,
}

/// The only platform this app ships on (DEVELOPMENT.md §14).
///
/// A manifest with no entry for it is not an error: it is what any build made
/// off Windows sees, and what a release that skipped the Windows bundle would
/// serve. "Nothing for you" and "something went wrong" are different answers.
pub const PLATFORM: &str = "windows-x86_64";

/// What this build is, for comparing against a manifest.
///
/// From Cargo rather than from `tauri.conf.json`: the two are kept in step by
/// the release workflow, and this is the one the binary actually carries.
pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Whether `offered` is a version worth telling someone about.
///
/// Semver rather than string comparison, because `0.10.0` sorts before `0.9.0`
/// as text and that would silently stop offering updates the first time a minor
/// version reached double digits.
///
/// Unparseable on either side answers `false`. A version we cannot read is not
/// one to invite somebody to install, and the alternative — treating "unknown"
/// as "newer" — offers an update to every user on every check.
pub fn is_newer(offered: &str, current: &str) -> bool {
    let (Ok(offered), Ok(current)) = (
        semver::Version::parse(offered.trim_start_matches('v')),
        semver::Version::parse(current.trim_start_matches('v')),
    ) else {
        return false;
    };
    offered > current
}

/// Reads a manifest and says what it means for this build.
///
/// The whole of the check's decision half, with the network on one side of it
/// and `UpdateStatus` on the other.
pub fn evaluate(body: &str, current: &str) -> CheckResult {
    let manifest: Manifest = match serde_json::from_str(body) {
        Ok(manifest) => manifest,
        Err(e) => return CheckResult::Failed(format!("Could not read the update manifest: {e}")),
    };

    // No entry for us is "nothing newer", not a failure. See `PLATFORM`.
    if !manifest.platforms.contains_key(PLATFORM) {
        return CheckResult::NothingNewer;
    }
    if !is_newer(&manifest.version, current) {
        return CheckResult::NothingNewer;
    }

    CheckResult::Found(UpdateOffer {
        version: manifest.version,
        notes: manifest.notes,
        pub_date: manifest.pub_date,
    })
}

#[cfg(test)]
mod manifest_tests {
    use super::*;

    fn manifest(version: &str) -> String {
        format!(
            r#"{{"version":"{version}","notes":"n","pub_date":"d",
               "platforms":{{"windows-x86_64":{{"signature":"s","url":"u"}}}}}}"#
        )
    }

    #[test]
    fn a_newer_version_is_offered() {
        let found = evaluate(&manifest("2.1.0"), "2.0.0");
        match found {
            CheckResult::Found(offer) => {
                assert_eq!(offer.version, "2.1.0");
                assert_eq!(offer.notes.as_deref(), Some("n"));
            }
            other => panic!("expected an offer, got {other:?}"),
        }
    }

    #[test]
    fn the_same_version_is_nothing_newer() {
        assert!(matches!(evaluate(&manifest("2.0.0"), "2.0.0"), CheckResult::NothingNewer));
    }

    #[test]
    fn an_older_version_is_nothing_newer() {
        assert!(matches!(evaluate(&manifest("1.9.9"), "2.0.0"), CheckResult::NothingNewer));
    }

    /// The bug string comparison would have shipped: `0.10.0` sorts before
    /// `0.9.0` as text, so the first double-digit minor version would have
    /// silently stopped offering updates to everyone.
    #[test]
    fn ten_is_newer_than_nine() {
        assert!(is_newer("0.10.0", "0.9.0"));
        assert!(is_newer("2.10.0", "2.9.5"));
        assert!(!is_newer("0.9.0", "0.10.0"));
    }

    /// A manifest with no entry for this platform is what a build made off
    /// Windows sees, and what a release that skipped the Windows bundle would
    /// serve. Not an error to report to anybody.
    #[test]
    fn no_entry_for_this_platform_is_nothing_newer() {
        let body = r#"{"version":"9.9.9","platforms":{"linux-x86_64":{"signature":"s","url":"u"}}}"#;
        assert!(matches!(evaluate(body, "2.0.0"), CheckResult::NothingNewer));
    }

    #[test]
    fn a_manifest_that_is_not_json_fails_rather_than_offering() {
        assert!(matches!(evaluate("<html>404</html>", "2.0.0"), CheckResult::Failed(_)));
    }

    /// Treating "unknown" as newer would offer an update to every user on
    /// every check, forever.
    #[test]
    fn an_unreadable_version_is_not_newer() {
        assert!(!is_newer("not-a-version", "2.0.0"));
        assert!(!is_newer("2.0.0", "also-not"));
    }

    /// Fields a newer client adds must not stop an older one checking.
    #[test]
    fn unknown_fields_are_ignored() {
        let body = r#"{"version":"9.9.9","surprise":true,
                       "platforms":{"windows-x86_64":{"signature":"s","url":"u","extra":1}}}"#;
        assert!(matches!(evaluate(body, "2.0.0"), CheckResult::Found(_)));
    }

    /// The version in the binary has to be readable by the comparison that
    /// decides whether to offer an update, or nothing is ever offered.
    /// The one URL written in two places, pinned against the file Tauri reads.
    ///
    /// A mismatch would not crash: the daemon would check one endpoint while
    /// the bundle advertised another, and updates would simply stop arriving
    /// with nothing to show for it.
    #[test]
    fn the_stable_endpoint_matches_tauri_conf() {
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let configured = conf["plugins"]["updater"]["endpoints"][0].as_str();
        assert_eq!(
            configured,
            Some(STABLE_ENDPOINT),
            "update::STABLE_ENDPOINT and tauri.conf.json name the same manifest"
        );
    }

    #[test]
    fn this_builds_own_version_parses() {
        assert!(semver::Version::parse(current_version()).is_ok(), "{}", current_version());
    }
}
