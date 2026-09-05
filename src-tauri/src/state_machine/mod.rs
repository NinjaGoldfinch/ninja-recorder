//! The game state machine that drives recording from League Client and
//! Live Client Data events. DEVELOPMENT.md §3.4.

pub mod machine;
pub mod supervisor;

pub use supervisor::{Supervisor, SupervisorStatus};

// Re-exported for consumers outside this module (the dev portal's state
// injection, tests elsewhere); `machine`'s own tests and `supervisor`
// reach these via `super::`.
#[allow(unused_imports)]
pub use machine::{Action, GameState, StateEvent, StateMachine};
#[allow(unused_imports)]
pub use supervisor::{FinalizedRecording, SessionMarker};
#[cfg(feature = "devtools")]
pub use supervisor::DevSessionView;
