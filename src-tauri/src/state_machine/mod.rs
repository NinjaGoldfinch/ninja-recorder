//! The game state machine that drives recording from League Client and
//! Live Client Data events. DEVELOPMENT.md §3.4.

pub mod machine;
pub mod supervisor;

// `SupervisorEvent` is the daemon's now: `lib.rs` used to match on it to raise
// notifications, and the whole notifier went with the supervisor in WS3.2.
// Re-exported still because it is part of this module's surface and the daemon
// reaches it through here.
#[cfg_attr(not(test), allow(unused_imports))]
pub use supervisor::{Supervisor, SupervisorEvent, SupervisorStatus};

// Re-exported for consumers outside this module (the dev portal's state
// injection, tests elsewhere); `machine`'s own tests and `supervisor`
// reach these via `super::`.
#[allow(unused_imports)]
pub use machine::{Action, GameState, StateEvent, StateMachine};
#[allow(unused_imports)]
pub use supervisor::{FinalizedRecording, SessionMarker};
#[cfg(feature = "devtools")]
pub use supervisor::DevSessionView;
