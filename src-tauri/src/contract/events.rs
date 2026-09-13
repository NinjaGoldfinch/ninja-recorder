//! The event half of the contract.
//!
//! **Empty — WS2 task 2.3.** `#[derive(ContractEvent)] enum Event`, with the
//! draft variants in the implementation plan's Appendix B. The seam it hangs
//! off already exists: `state_machine`'s `Action` list is what the supervisor
//! already emits, so the enum is a naming exercise rather than a new concept
//! (implementation plan §4.1).
//!
//! `Event::LcuPhase` is the one variant with an open question — whether it
//! carries the full `gameflow-phase` enumeration or the subset
//! `lcu/gameflow.rs` models today. The LCU swagger is the authoritative list;
//! see the plan's §9.
//!
//! A generated `event_names()` used only by tests needs the same
//! `cfg_attr(allow(dead_code))` pattern `core::command_names()` already uses,
//! because clippy runs without `--all-targets` (see `CLAUDE.md`).
