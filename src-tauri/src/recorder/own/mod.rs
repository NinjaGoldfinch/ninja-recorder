//! The own capture backend — WGC → D3D11 → Media Foundation.
//!
//! **Empty — WS1 task 1.6 (P1), gated on the P0c spike (tasks 1.3–1.5).**
//! This is Option B, the *target* backend: it is what removes libobs, and
//! removing libobs is what allows the licence to change at v2.1. It plugs in
//! behind the `Recorder` trait in `recorder/mod.rs` exactly as
//! `recorder/libobs/` and `recorder/stub.rs` do, and nothing above the trait
//! learns which one is linked.
//!
//! Stages, in the order the spike proves them (implementation plan §4.5):
//!
//! 1. Windows.Graphics.Capture for frames — no injection, ever. That is a hard
//!    constraint, not a preference (DEVELOPMENT.md §1.1).
//! 2. D3D11 for the texture path.
//! 3. Media Foundation `SinkWriter` for H.264/AAC into fragmented MP4.
//! 4. Process loopback for per-application game audio — the stage the P0c
//!    gate is really about, and the one that decides whether Option B can
//!    replace libobs rather than merely join it.
//!
//! `recorder/libobs/` stays as the fallback and as a selectable second backend
//! for exactly one release. WS8 deletes it.
