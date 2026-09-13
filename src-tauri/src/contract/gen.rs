//! TypeScript emitter for the contract.
//!
//! **Empty — WS2 task 2.4.** Walks `contract_manifest()` and the `ts-rs`
//! registrations and writes `src/lib/contract/{types,client,events}.ts`. Run as
//! `cargo run --bin gen-contract`; `--check` re-emits into a buffer and fails
//! if it differs from what is committed, which is the CI gate (task 2.5).
//!
//! The output is committed rather than generated at build time, so a frontend
//! developer with no Rust toolchain can still work and so the diff of a
//! contract change is reviewable (implementation plan §4.1).
