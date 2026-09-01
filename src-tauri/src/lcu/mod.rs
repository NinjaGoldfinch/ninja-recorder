//! League Client (LCU) integration. DEVELOPMENT.md §3.1, §3.3.
//!
//! Two entry points matter to the rest of the app: `lockfile::discover` /
//! `lockfile::watch` to find the running client, and `gameflow::watch` to
//! track game state once connected. `match_data` pulls post-game stats for
//! VOD metadata.

pub mod client;
pub mod gameflow;
pub mod lockfile;
pub mod match_data;

pub use client::LcuHttpClient;
pub use gameflow::GameflowPhase;

#[allow(unused_imports)]
pub use client::LcuClientError;

// Re-exported for future consumers (Phase 3 state machine, Phase 4 VOD
// library) — not all used internally yet.
#[allow(unused_imports)]
pub use gameflow::{GameflowSource, GameflowUpdate};
#[allow(unused_imports)]
pub use lockfile::{LockfileError, LockfileInfo, LockfileState};
#[allow(unused_imports)]
pub use match_data::{fetch_match_summary, MatchDataError, MatchSummary};
