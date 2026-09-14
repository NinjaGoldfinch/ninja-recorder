//! The event half of the contract — WS2 task 2.3.
//!
//! v1 has no declaration of this half at all. Commands are declared once in
//! `core::dispatch`'s `dispatch_table!`; events were whatever `lib.rs` happened
//! to pass to `Emitter::emit`, named by string constants at the call site and
//! mirrored by hand in TypeScript. This module is the other half of the same
//! idea: one table, from which the enum, the subscription topic, the name list
//! and the generator's manifest are all derived.
//!
//! ## Why a `macro_rules!` and not a derive
//!
//! The plan's Appendix B calls `ContractEvent` "a small derive". A real derive
//! needs a proc-macro crate, which would mean turning `src-tauri` into a
//! workspace and taking `syn`, `quote` and `proc-macro2` — three dependencies
//! and a restructure — to produce output a declarative macro produces already.
//! `dispatch_table!` is the exact precedent: it declares the command half and
//! emits `command_names()` and `contract_manifest()` beside it. This is that
//! shape, for events. The plan's *output* is unchanged; only the mechanism is.
//!
//! ## The one open question, answered by the code
//!
//! Issue #73 (Q7) asks whether `Event::LcuPhase` carries the full
//! `gameflow-phase` enumeration or "the subset `lcu/gameflow.rs` models today".
//! There is no subset: `GameflowPhase` already names all fourteen phases the
//! LCU defines and carries `Unknown(String)` for anything a client update
//! invents. The state machine *consumes* a subset — `is_game_running_phase`
//! matches two variants — but that is a reader narrowing a full type, not a
//! narrow type. So this carries `GameflowPhase` whole, and a phase nothing
//! recognises still reaches the UI as `Unknown` rather than being dropped.

use serde::Serialize;

/// The subscription key. A client asks for topics, not for individual events,
/// so a variant added to an existing topic reaches every subscriber that
/// already wanted that kind of thing without a client change.
///
/// Five, fixed, and deliberately not derived from the event table: the set is
/// part of the contract's shape, and a topic that existed only because some
/// event named it would appear and vanish as events came and went.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub enum Topic {
    /// The state machine, the recorder, and everything that happens during one
    /// game.
    Recording,
    /// Whether the client is there, and what it says it is doing.
    Lcu,
    /// The VOD library changed behind the frontend's back.
    Library,
    /// Update check and install progress.
    Update,
    /// The daemon's own lifecycle, and the transport's health. The main UI
    /// subscribes to everything but this.
    Daemon,
}

/// Why the library changed. A reason rather than a bare ping so a client can
/// decide whether a full refetch is warranted — reconcile and retention can
/// both move many rows at once, while a patch touches one.
/// Dead until the sink that emits it lands (the second half of WS2.3), and
/// clippy runs without `--all-targets` — same treatment, same reason, as
/// `Event` above.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub enum LibraryChangeReason {
    /// A recording finished and was written.
    Finalized,
    /// A row was edited or deleted through a command.
    Edited,
    /// `db::reconcile` imported or forgot files after a scan.
    Reconciled,
    /// `retention` deleted rows to get back under the cap.
    Retention,
}

/// How a recording ended. `Refused` is not an error: the state machine declines
/// to record spectator sessions and reconnects to an already-recorded game, and
/// a client that cannot tell that from a crash will show the wrong thing.
/// Dead until the sink that emits it lands (the second half of WS2.3), and
/// clippy runs without `--all-targets` — same treatment, same reason, as
/// `Event` above.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ts_rs::TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum StopOutcome {
    /// Finalized, muxed and written.
    Clean,
    /// The recorder or the game went away mid-capture. Whatever was written
    /// before that point is still on disk.
    Crashed,
    /// Never started, and why.
    #[serde(rename_all = "camelCase")]
    Refused { reason: String },
}

/// Why the daemon is going away. The UI uses this to decide whether to say
/// anything: a quit the user asked for needs no announcement, an update install
/// does, and an error certainly does.
/// Dead until the sink that emits it lands (the second half of WS2.3), and
/// clippy runs without `--all-targets` — same treatment, same reason, as
/// `Event` above.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub enum ShutdownReason {
    /// The user chose Quit.
    Quit,
    /// An update is being installed; the installer replaces the binary.
    Update,
    /// Something unrecoverable. The message is fit to show a user.
    Error,
}

/// One event's shape as *data*, for the generator — the event-half twin of
/// `core::dispatch::CommandSpec`.
///
/// The strings come from `stringify!`, so the table's spelling is the
/// contract's spelling. Every payload type is written as an absolute `crate::`
/// path for the same reason the command table's are: a generator reading this
/// has no module context to resolve a bare name against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventSpec {
    pub name: &'static str,
    pub topic: Topic,
    pub fields: &'static [EventField],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventField {
    pub name: &'static str,
    pub ty: &'static str,
}

/// Declares the event half once: the enum, its topic mapping, the name list and
/// the generator's manifest all come out of this table.
///
/// Each row is `Variant { field: Type, … } => Topic`. Adding a variant without
/// a topic is a syntax error, which is the property that makes this a contract
/// rather than two lists — the thing `every_command_round_trips` exists to
/// enforce for commands at runtime, enforced here at compile time instead.
macro_rules! contract_events {
    (
        $(
            $(#[$meta:meta])*
            $variant:ident {
                $( $(#[$fmeta:meta])* $field:ident : $ty:ty ),* $(,)?
            } => $topic:ident
        ),* $(,)?
    ) => {
        /// Everything the daemon pushes. One enum, internally tagged, so the
        /// wire carries `{"type":"stateChanged",…}` and a TypeScript client
        /// gets a discriminated union it can exhaustively switch on.
        ///
        /// Nothing constructs one yet — the `EventSink` that emits these is
        /// the second half of WS2.3. Until then this is dead code in a build
        /// without `cfg(test)`, and clippy runs without `--all-targets`, so
        /// `-D warnings` would fail on it.
        #[cfg_attr(not(test), allow(dead_code))]
        #[derive(Debug, Clone, Serialize, ts_rs::TS)]
        #[serde(tag = "type", rename_all = "camelCase")]
        pub enum Event {
            $(
                $(#[$meta])*
                #[serde(rename_all = "camelCase")]
                $variant { $( $(#[$fmeta])* $field : $ty ),* },
            )*
        }

        #[cfg_attr(not(test), allow(dead_code))]
        impl Event {
            /// Which topic this event is delivered on. Total by construction:
            /// the match is generated from the same table as the variants, so
            /// it cannot fall out of step with them.
            pub fn topic(&self) -> Topic {
                match self {
                    $( Event::$variant { .. } => Topic::$topic, )*
                }
            }
        }

        /// Every event name, in declaration order.
        ///
        /// These are the **Rust** variant names. The wire names are those
        /// camelCased by `rename_all`, and `wire_names_match_the_declaration`
        /// below ties the two together so the pair cannot drift silently.
        ///
        /// Nothing outside the tests calls this yet, and clippy runs without
        /// `--all-targets`, so in a shipped build it is genuinely dead code and
        /// `-D warnings` would fail on it — the same treatment, for the same
        /// reason, as `core::dispatch::contract_manifest`.
        #[cfg_attr(not(test), allow(dead_code))]
        pub fn event_names() -> &'static [&'static str] {
            &[ $( stringify!($variant), )* ]
        }

        /// The event surface as data. WS2.5's `gen-contract` is its first real
        /// consumer; the tests below are the only one today.
        #[cfg_attr(not(test), allow(dead_code))]
        pub fn event_manifest() -> &'static [EventSpec] {
            &[
                $(
                    EventSpec {
                        name: stringify!($variant),
                        topic: Topic::$topic,
                        fields: &[
                            $( EventField { name: stringify!($field), ty: stringify!($ty) }, )*
                        ],
                    },
                )*
            ]
        }
    };
}

contract_events! {
    /// Every `StateMachine::handle` that actually changes state. Not emitted
    /// for a handled event that left the state where it was — a client
    /// redrawing on those would redraw on every gameflow poll.
    StateChanged {
        state: crate::state_machine::machine::GameState,
        /// Epoch milliseconds at which this state was entered, so a client can
        /// render "recording for 4:12" without timing it itself and without
        /// drifting when the window was asleep.
        since_ms: i64,
    } => Recording,

    /// The client appeared, vanished, or said it was doing something else.
    ///
    /// `phase` is `None` exactly when `client_present` is false: with no client
    /// there is no phase, which is a different statement from `Phase::None`
    /// (a client sitting at the front page).
    LcuPhase {
        phase: Option<crate::lcu::gameflow::GameflowPhase>,
        client_present: bool,
    } => Lcu,

    /// `Recorder::start` returned `Ok`.
    RecordingStarted {
        /// `None` until finalize: the library row is written when the recording
        /// ends, so nothing has an id while it is still being captured. A
        /// client correlates on `path` until `RecordingStopped` names the row.
        recording_id: Option<i64>,
        path: String,
        started_at_ms: i64,
    } => Recording,

    /// Finalize completed, or capture ended without ever starting.
    RecordingStopped {
        recording_id: Option<i64>,
        outcome: crate::contract::events::StopOutcome,
    } => Recording,

    /// `MarkerTracker` produced one, already positioned against the video.
    MarkerAdded {
        recording_id: Option<i64>,
        marker: crate::state_machine::supervisor::SessionMarker,
    } => Recording,

    /// The 1 Hz advantage samples, batched.
    ///
    /// Batched rather than pushed individually because a 35-minute game
    /// produces about 2,000 of them, and 2,000 notifications is a frame-rate
    /// problem in the UI for data that is drawn as one line.
    SampleBatch {
        recording_id: Option<i64>,
        samples: Vec<crate::state_machine::supervisor::SessionSample>,
    } => Recording,

    /// The deferred LCU post-game patch landed on a row.
    MatchSummaryPatched { recording_id: i64 } => Library,

    /// Any row mutation, reconcile pass or retention run.
    LibraryChanged {
        reason: crate::contract::events::LibraryChangeReason,
    } => Library,

    /// One retention enforcement pass, whether or not it deleted anything.
    RetentionRan { deleted: Vec<i64>, freed_bytes: i64 } => Library,

    /// The update status transitioned. Named after Appendix B's variant; the
    /// payload is the same `UpdateStatus` the About block already renders.
    UpdateStatus {
        status: crate::update::UpdateStatus,
    } => Update,

    /// Sent before the daemon starts tearing down, so a UI can say why it is
    /// about to lose its connection instead of reporting a dead pipe.
    DaemonShuttingDown {
        reason: crate::contract::events::ShutdownReason,
    } => Daemon,

    /// The client fell behind the broadcast buffer and events were dropped.
    /// The transport handles this by re-`hello`ing rather than showing it.
    Lagged { dropped: u32 } => Daemon,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::types::config;
    use crate::lcu::gameflow::GameflowPhase;
    use std::collections::BTreeSet;
    use ts_rs::TS;

    /// serde's `rename_all = "camelCase"` applied to a variant name, so the
    /// test can predict the wire tag from the Rust identifier.
    fn camel(pascal: &str) -> String {
        let mut chars = pascal.chars();
        match chars.next() {
            Some(first) => first.to_lowercase().collect::<String>() + chars.as_str(),
            None => String::new(),
        }
    }

    /// The table is the contract, so its size is worth stating out loud: a
    /// variant appearing or disappearing should be a deliberate diff, not a
    /// silent one.
    #[test]
    fn the_event_surface_is_the_twelve_variants_appendix_b_names() {
        assert_eq!(event_names().len(), 12);
        assert_eq!(event_manifest().len(), 12);
    }

    /// Two lists generated from one table cannot disagree — but they can both
    /// be wrong in the same way, so this checks they are the *same* list rather
    /// than merely the same length.
    #[test]
    fn the_manifest_and_the_name_list_describe_the_same_events() {
        let from_manifest: Vec<&str> = event_manifest().iter().map(|s| s.name).collect();
        assert_eq!(from_manifest, event_names());
    }

    #[test]
    fn every_event_name_is_unique() {
        let unique: BTreeSet<&&str> = event_names().iter().collect();
        assert_eq!(unique.len(), event_names().len(), "duplicate event name");
    }

    /// The generated `topic()` is total by construction; what this checks is
    /// that the table actually *uses* all five topics. A topic nothing is
    /// delivered on is a subscription a client can make and never hear from,
    /// which is worse than not offering it.
    #[test]
    fn every_topic_carries_at_least_one_event() {
        let used: BTreeSet<Topic> = event_manifest().iter().map(|s| s.topic).collect();
        for topic in [
            Topic::Recording,
            Topic::Lcu,
            Topic::Library,
            Topic::Update,
            Topic::Daemon,
        ] {
            assert!(used.contains(&topic), "no event is delivered on {topic:?}");
        }
        assert_eq!(used.len(), 5);
    }

    /// `topic()` and the manifest are generated from one table, so this is
    /// really asserting the macro expands the two arms consistently — which is
    /// the whole reason the mapping is not hand-written.
    #[test]
    fn the_runtime_topic_matches_the_manifest() {
        let lagged = Event::Lagged { dropped: 3 };
        assert_eq!(lagged.topic(), Topic::Daemon);

        let spec = event_manifest()
            .iter()
            .find(|s| s.name == "Lagged")
            .expect("Lagged is declared");
        assert_eq!(spec.topic, lagged.topic());
    }

    /// The bridge between `event_names()` (Rust identifiers) and the wire.
    ///
    /// `event_names` cannot return camelCase — `macro_rules!` has no way to
    /// change the case of an identifier — so the two could drift if serde's
    /// `rename_all` were ever removed or changed. This makes that a test
    /// failure rather than a client that silently stops matching any event.
    #[test]
    fn wire_names_match_the_declaration() {
        let decl = <Event as TS>::decl(&config());
        for name in event_names() {
            let wire = camel(name);
            assert!(
                decl.contains(&format!(r#""{wire}""#)),
                "the TypeScript has no `\"{wire}\"` tag for {name}:\n{decl}"
            );
        }
    }

    /// A tagged enum must render as a union of objects, not a bare string
    /// union: every variant here carries a payload, and a client switching on
    /// `type` needs the payload to come with it.
    #[test]
    fn the_event_renders_as_a_discriminated_union() {
        let decl = <Event as TS>::decl(&config());
        assert!(decl.contains(r#""type""#), "the tag is missing:\n{decl}");
        assert!(decl.contains('|'), "not a union:\n{decl}");
        assert!(
            !decl.contains("bigint"),
            "JSON cannot carry a BigInt:\n{decl}"
        );
    }

    /// Q7, made executable. The LCU's own list is fourteen phases plus whatever
    /// a future client invents, and the event carries all of it — so a phase
    /// the state machine ignores still reaches a client that wants to show it.
    #[test]
    fn lcu_phase_carries_the_whole_enumeration_including_the_catch_all() {
        let decl = <GameflowPhase as TS>::decl(&config());
        for phase in [
            "None",
            "Lobby",
            "Matchmaking",
            "CheckedIntoTournament",
            "ReadyCheck",
            "ChampSelect",
            "GameStart",
            "FailedToLaunch",
            "InProgress",
            "Reconnect",
            "WaitingForStats",
            "PreEndOfGame",
            "EndOfGame",
            "TerminatedInError",
        ] {
            assert!(
                decl.contains(phase),
                "GameflowPhase lost `{phase}`:\n{decl}"
            );
        }
        assert!(
            decl.contains("Unknown"),
            "the catch-all is what keeps an unrecognised phase from being dropped:\n{decl}"
        );
    }

    /// `phase: None` and `Phase::None` are different statements and the enum
    /// has to keep them apart — "no client" versus "a client sitting at the
    /// front page". This is a compile-level assertion that the field is
    /// optional at all.
    #[test]
    fn no_client_is_distinct_from_the_none_phase() {
        let absent = Event::LcuPhase {
            phase: None,
            client_present: false,
        };
        let front_page = Event::LcuPhase {
            phase: Some(GameflowPhase::None),
            client_present: true,
        };
        let absent = serde_json::to_value(&absent).unwrap();
        let front_page = serde_json::to_value(&front_page).unwrap();
        assert_ne!(absent, front_page);
        assert_eq!(absent["phase"], serde_json::Value::Null);
        assert_eq!(front_page["phase"], serde_json::json!("None"));
    }

    /// The tag is what a client switches on, so its spelling is load-bearing in
    /// a way the rest of the payload is not.
    #[test]
    fn the_wire_shape_is_what_a_client_switches_on() {
        let event = Event::RetentionRan {
            deleted: vec![1, 2],
            freed_bytes: 4096,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "retentionRan");
        assert_eq!(json["deleted"], serde_json::json!([1, 2]));
        assert_eq!(json["freedBytes"], 4096);
    }

    /// `Refused` carries a reason and the other two do not, which is the whole
    /// point of it being an enum rather than a boolean.
    #[test]
    fn a_refusal_is_not_a_crash() {
        let refused = serde_json::to_value(StopOutcome::Refused {
            reason: "spectator session".into(),
        })
        .unwrap();
        assert_eq!(refused["kind"], "refused");
        assert_eq!(refused["reason"], "spectator session");

        let crashed = serde_json::to_value(StopOutcome::Crashed).unwrap();
        assert_eq!(crashed["kind"], "crashed");
        assert!(crashed.get("reason").is_none());
    }

    /// Payload types are spelled as absolute `crate::` paths so a generator
    /// with no module context can resolve them. Easy to violate by writing the
    /// short name the `use` above already provides.
    #[test]
    fn every_payload_type_is_an_absolute_path() {
        /// Everything that is not a project type, and so needs no path.
        const PRIMITIVE: &[&str] = &[
            "i64",
            "u32",
            "bool",
            "String",
            "f64",
            "Option<i64>",
            "Vec<i64>",
        ];

        for spec in event_manifest() {
            for field in spec.fields {
                assert!(
                    PRIMITIVE.contains(&field.ty) || field.ty.contains("crate::"),
                    "{}.{} is `{}`; write it as an absolute crate:: path",
                    spec.name,
                    field.name,
                    field.ty
                );
            }
        }
    }
}
