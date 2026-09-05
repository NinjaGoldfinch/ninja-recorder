//! Live Client Data integration: the in-game event stream used for
//! marker extraction. DEVELOPMENT.md §3.2, §3.4.

pub mod client;
pub mod events;
pub mod poller;

pub use client::LiveClientDataClient;
pub use events::{team_diff, AllGameData, Marker, MarkerTracker, TeamDiff, TimeAlignment};

// Re-exported for consumers outside this module (the supervisor, the dev
// portal) — not used internally.
#[allow(unused_imports)]
pub use client::LiveClientError;
#[allow(unused_imports)]
pub use events::MarkerKind;
