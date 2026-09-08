//! League Client (LCU) integration. DEVELOPMENT.md §3.1, §3.3.
//!
//! Two entry points matter to the rest of the app: `lockfile::discover` /
//! `lockfile::watch` to find the running client, and `gameflow::watch` to
//! track game state once connected. `match_data` pulls post-game stats for
//! VOD metadata, `champions` turns the champion id it answers with into the
//! display name the library sorts on, and `timeline` reads the gold series
//! back out of Riot's own per-participant accounting.

pub mod champions;
pub mod client;
pub mod gameflow;
pub mod lockfile;
pub mod match_data;
pub mod timeline;

pub use client::LcuHttpClient;
pub use gameflow::{fetch_session, GameflowPhase, GameIdentity};

#[allow(unused_imports)]
pub use client::LcuClientError;

// Re-exported for consumers outside this module (the state machine, the
// dev portal) — not all used internally.
#[allow(unused_imports)]
pub use gameflow::{GameflowSource, GameflowUpdate};
#[allow(unused_imports)]
pub use lockfile::{LockfileError, LockfileInfo, LockfileState};
#[allow(unused_imports)]
pub use match_data::{
    fetch_match_summary, fetch_recent_games, fetch_participants, fetch_sides, MatchDataError, MatchSummary,
    ParticipantSummary, PlayedGame, Sides,
};
// `GoldPoint` is deliberately not re-exported: the one caller never names
// it, and `-D warnings` fails on a re-export nothing uses — this line has no
// `allow` above it, unlike the group of external re-exports. It stays public
// on `timeline` for whoever does name it.
pub use timeline::fetch_gold_series;
pub use champions::champion_name;
