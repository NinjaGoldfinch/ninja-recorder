//! Live Client Data integration: the in-game event stream used for
//! marker extraction. DEVELOPMENT.md §3.2, §3.4.

pub mod client;
pub mod events;
pub mod poller;

pub use client::LiveClientDataClient;
pub use events::{team_diff, AllGameData, Marker, MarkerTracker, TeamDiff, TimeAlignment};

// Re-exported for future consumers (Phase 4 VOD library, Phase 5 review
// UI) — not used internally yet.
#[allow(unused_imports)]
pub use client::LiveClientError;
#[allow(unused_imports)]
pub use events::MarkerKind;
