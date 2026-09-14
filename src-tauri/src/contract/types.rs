//! How Rust types are rendered as TypeScript — WS2.2.
//!
//! Every type that crosses the IPC boundary derives `ts_rs::TS` where it is
//! defined, next to its `serde` derives, because the two describe the same wire
//! shape and splitting them is how they drift. This module holds the one thing
//! that cannot live on the type: the configuration those declarations are
//! rendered under.
//!
//! **`#[ts(export)]` is deliberately not used anywhere.** It writes a `.ts` file
//! per type as a side effect of `cargo test`, which scatters the output and
//! makes it a test artifact. WS2.5's `gen-contract` collects the declarations
//! instead, so emission happens once, in one place, reviewable as one diff.

use ts_rs::Config;

/// The configuration every declaration in this project is rendered under.
///
/// One function rather than a default, because the default is wrong for us in a
/// way that compiles perfectly and only shows up at runtime — see below.
///
/// WS2.5's generator must use this. So must any test that asserts on a
/// declaration, or the test proves something about a shape nobody ships.
///
/// Dead in a shipped build until `gen-contract` exists, and clippy runs without
/// `--all-targets` — same treatment, and the same reason, as
/// `core::dispatch::contract_manifest`.
#[cfg_attr(not(test), allow(dead_code))]
pub fn config() -> Config {
    // **`number`, not ts-rs's default of `bigint`.**
    //
    // ts-rs maps `i64`/`u64`/`i128`/`u128` to `bigint` on the reasonable
    // grounds that they do not fit losslessly in a float64. That reasoning is
    // about Rust and JavaScript; it is not about *this wire format*.
    //
    // What actually crosses the pipe is JSON. `serde_json` writes an `i64` as a
    // JSON number, and `JSON.parse` hands back a `number` — never a `BigInt`,
    // which JSON cannot represent at all. So a generated `id: bigint` would
    // describe a value the runtime never produces, and every `id` in the app
    // would be a type error against a correct implementation.
    //
    // `src/types.ts` already says `number` for all of these, and has been right
    // in production since v1.
    //
    // The precision argument does not bite here: the largest values on this
    // boundary are `started_at` (epoch milliseconds, ~1.7e12) and Riot game ids
    // (~1e10), both far below 2^53. If a genuinely larger integer ever joins
    // the boundary, it needs a string on the wire, not a `bigint` in a type
    // that JSON cannot carry.
    Config::new().with_large_int("number")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ts_rs::TS;

    /// Every type on the boundary, named once, so that "it compiles" is not
    /// mistaken for "it renders". A `TS` derive can compile and still produce
    /// an empty or degenerate declaration; this walks all of them.
    ///
    /// The list is hand-written, which is exactly the kind of second list WS2
    /// exists to delete — but the manifest names types as *strings*
    /// (`"Vec<crate::db::RecordingRow>"`), and a string cannot be turned back
    /// into a Rust type to ask it for its declaration. Closing that gap is
    /// WS2.5's, since it is the generator that needs the mapping. Until then
    /// this list is the honest version of the problem rather than a hidden one.
    macro_rules! all_boundary_types {
        ($m:ident) => {
            $m!(
                crate::backfill::BackfillReport,
                crate::contract::events::Event,
                crate::contract::events::LibraryChangeReason,
                crate::contract::events::ShutdownReason,
                crate::contract::events::StopOutcome,
                crate::contract::events::Topic,
                crate::core::AutostartStatus,
                crate::core::DiskUsage,
                crate::core::LcuStatus,
                crate::db::MarkerRow,
                crate::db::RecordingRow,
                crate::db::RetentionPolicy,
                crate::db::SampleRow,
                crate::db::reconcile::ReconcileReport,
                crate::ddragon::IconRequest,
                crate::ddragon::IconSet,
                crate::lcu::gameflow::GameflowPhase,
                crate::live_client::events::Marker,
                crate::live_client::events::MarkerKind,
                crate::live_client::events::Scoreboard,
                crate::live_client::events::ScoreboardPlayer,
                crate::live_client::events::ScoreboardRunes,
                crate::live_client::events::TeamDiff,
                crate::recorder::audio::AudioInputDevice,
                crate::recorder::audio::AudioLayout,
                crate::recorder::audio::AudioPreset,
                crate::recorder::audio::AudioSourceKind,
                crate::recorder::audio::AudioTrackSpec,
                crate::retention::EnforcementReport,
                crate::state_machine::machine::GameState,
                crate::state_machine::supervisor::FinalizedRecording,
                crate::state_machine::supervisor::RecordingDiagnostics,
                crate::state_machine::supervisor::SessionMarker,
                crate::state_machine::supervisor::SessionSample,
                crate::state_machine::supervisor::SupervisorStatus,
                crate::update::UpdateOffer,
                crate::update::UpdateStatus,
            )
        };
    }

    #[test]
    fn every_boundary_type_renders_a_usable_declaration() {
        let cfg = config();
        macro_rules! check {
            ($($t:ty),* $(,)?) => {{
                let mut n = 0;
                $(
                    let name = <$t as TS>::name(&cfg);
                    let decl = <$t as TS>::decl(&cfg);
                    assert!(!name.is_empty(), "{} has no TS name", stringify!($t));
                    assert!(
                        decl.len() > name.len() + "type  = ".len(),
                        "{} renders a degenerate declaration: {decl}",
                        stringify!($t)
                    );
                    assert!(
                        !decl.contains("bigint"),
                        "{} contains bigint, which JSON cannot carry: {decl}",
                        stringify!($t)
                    );
                    n += 1;
                )*
                n
            }};
        }
        let n = all_boundary_types!(check);
        assert_eq!(n, 37, "the boundary type list changed; update the count deliberately");
    }

    /// The decision above, made executable. Rendering with ts-rs's default
    /// would produce `bigint` here and nothing would fail to compile — this is
    /// the only thing standing between that and a generated client that is
    /// wrong about every id in the app.
    #[test]
    fn large_integers_render_as_number_because_json_has_no_bigint() {
        let cfg = config();
        let decl = <crate::db::RecordingRow as TS>::decl(&cfg);
        assert!(
            decl.contains("id: number"),
            "RecordingRow.id must be `number`, got:\n{decl}"
        );
        assert!(
            !decl.contains("bigint"),
            "no declaration may contain `bigint` — JSON cannot represent it:\n{decl}"
        );
    }

    /// `#[serde(tag = "...")]` makes an externally-tagged enum into an
    /// internally-tagged object, and the TypeScript has to describe the object
    /// serde actually writes rather than the Rust enum. This is what
    /// `serde-compat` buys, and it is a default feature that could be turned
    /// off by an innocent-looking `default-features = false`.
    #[test]
    fn serde_tagging_is_reflected_in_the_typescript() {
        let cfg = config();
        let decl = <crate::update::UpdateStatus as TS>::decl(&cfg);
        // ts-rs quotes the tag key, so this looks for `"kind"` rather than
        // `kind:` — the first draft of this test checked the unquoted form and
        // failed against output that was already correct.
        assert!(
            decl.contains(r#""kind""#),
            "UpdateStatus is #[serde(tag = \"kind\")]; the TS must carry the tag:\n{decl}"
        );
        // `rename_all = "camelCase"` has to reach the variant names too, or the
        // client compares against `UpToDate` while the daemon sends `upToDate`.
        assert!(
            decl.contains(r#""upToDate""#),
            "variant names must be camelCased as serde renames them:\n{decl}"
        );
        // A tagged enum must be a union of objects, not a bare string union.
        assert!(
            decl.contains('|'),
            "UpdateStatus should render as a discriminated union:\n{decl}"
        );
    }

    /// The one attribute ts-rs cannot parse, covered by assertion rather than
    /// by a warning: `#[serde(other)]` on `AudioPreset::Unknown`.
    ///
    /// `no-serde-warnings` is on, so if ts-rs ever started dropping that
    /// variant — or any other — nothing would say so. This would.
    #[test]
    fn audio_presets_keep_every_variant_including_the_catch_all() {
        let cfg = config();
        let decl = <crate::recorder::audio::AudioPreset as TS>::decl(&cfg);
        for variant in ["game", "game_mic", "game_mic_discord", "desktop", "custom", "unknown"] {
            assert!(
                decl.contains(&format!(r#""{variant}""#)),
                "AudioPreset lost the `{variant}` variant:\n{decl}"
            );
        }
        // `skip_serializing_if` makes the field genuinely optional on the wire.
        assert!(
            decl.contains("mic_device_id?:"),
            "mic_device_id is skip_serializing_if and must be optional:\n{decl}"
        );
    }

    /// `#[serde(flatten)]` lifts `Marker`'s fields into `SessionMarker`. If
    /// ts-rs emitted a nested `marker` object instead, the generated client
    /// would disagree with every marker the daemon sends.
    #[test]
    fn flattened_fields_are_lifted_not_nested() {
        let cfg = config();
        let decl = <crate::state_machine::supervisor::SessionMarker as TS>::decl(&cfg);
        assert!(
            !decl.contains("marker:"),
            "SessionMarker flattens Marker; a nested `marker` field means flatten was ignored:\n{decl}"
        );
        assert!(
            decl.contains("video_time_s"),
            "SessionMarker must keep its own fields:\n{decl}"
        );
    }
}

