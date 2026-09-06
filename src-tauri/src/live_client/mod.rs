//! Live Client Data integration: the in-game event stream used for
//! marker extraction. DEVELOPMENT.md §3.2, §3.4.

pub mod client;
pub mod events;
pub mod poller;

pub use client::LiveClientDataClient;
pub use events::{
    self_summary, team_diff, AlignmentTracker, AllGameData, LiveSummary, Marker, MarkerTracker,
    TeamDiff, TimeAlignment,
};

// Re-exported for consumers outside this module (the supervisor, the dev
// portal) — not used internally.
#[allow(unused_imports)]
pub use client::LiveClientError;
#[allow(unused_imports)]
pub use events::MarkerKind;
// `LiveSummary`'s own field type, so it belongs on the public surface even
// though the only thing naming it directly today is a test — and clippy
// runs without `--all-targets`, so a test-only use reads as unused.
#[allow(unused_imports)]
pub use events::Kda;
