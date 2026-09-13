//! The single declaration of the command and event surface.
//!
//! **Empty — WS2 (tasks 2.1–2.6).** Commands and events are declared once, in
//! Rust, and the TypeScript client is generated from that declaration. Today
//! the command half exists as `core::dispatch`'s `dispatch_table!` and the
//! TypeScript half is hand-written in `src/bridge.ts` and `src/dev/registry.ts`
//! — two lists that can disagree, which is why `every_command_round_trips`
//! exists at all.
//!
//! | File | WS2 task | What it becomes |
//! |---|---|---|
//! | `mod.rs` | 2.1 | Re-exports; `contract_manifest()` emitted by the `dispatch_table!` macro. `#[contract::command]` functions stay in `core` |
//! | `events.rs` | 2.3 | `#[derive(ContractEvent)] enum Event` — the event half, which v1 has no declaration of at all |
//! | `gen.rs` | 2.4 | The TypeScript emitter, run as `cargo run --bin gen-contract` |
//!
//! WS2.5 puts `cargo run --bin gen-contract -- --check` in CI, which is what
//! replaces `every_command_round_trips` and the dev portal's drift banner.
//! WS2 deletes that test; the import commit keeps it.
//!
//! Types crossing the boundary get `ts-rs` derives (task 2.2, ~40 of them),
//! collected by the generator rather than by `#[ts(export)]`.

pub mod events;
// `r#gen`, not `gen`: `gen` is a reserved keyword in edition 2024 (it is the
// generator syntax), so the plain identifier stopped compiling with the
// edition bump. The *file* stays `gen.rs`, because §3.4 of the plan draws it
// that way and the `gen-contract` binary name is unaffected — a bin name is
// not an identifier. WS2 may rename the module if `r#gen` at the use sites
// proves annoying; it is recorded here rather than renamed silently.
pub mod r#gen;
