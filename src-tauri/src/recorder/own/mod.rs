//! The own capture backend — WGC → D3D11 → Media Foundation.
//!
//! **Not constructible yet.** WS1.6 builds it in pieces (the plan is the
//! comment on #10), and nothing here is reachable from `recorder::backend`
//! until #236 wires the first of it in. This is Option B, the *target*
//! backend: it is what removes libobs, and removing libobs is what allows the
//! licence to change at v2.1. It plugs in behind the `Recorder` trait in
//! `recorder/mod.rs` exactly as `recorder/libobs/` and `recorder/stub.rs` do,
//! and nothing above the trait learns which one is linked.
//!
//! Split the way `spikes/p0c-video` is: the pure modules are compiled and
//! unit-tested on every platform, and only `win/` is gated to Windows.
//!
//! - `clock` — the video tick grid and the audio [`clock::Aligner`] that
//!   places packets on it.
//! - `pcm` — endpoint sample formats to stereo i16 for an encoder, or f32 for
//!   the mixer.
//! - `select` — which H.264 encoder to use (hardware first, the software MFT
//!   only as a marked fallback), and the Windows build floor.
//! - `win` — everything that calls Windows. Empty until #236.
//!
//! Stages, in the order the spike proved them (implementation plan §4.5):
//!
//! 1. Windows.Graphics.Capture for frames — no injection, ever. That is a hard
//!    constraint, not a preference (DEVELOPMENT.md §1.1).
//! 2. D3D11 for the texture path.
//! 3. Media Foundation for H.264/AAC into fragmented MP4.
//! 4. Process loopback for per-application game audio — P0c stage 1, proved
//!    by `spikes/p0c-audio`. The libobs fork calls the same Windows API, so
//!    its result decides whether *either* backend can isolate game audio,
//!    not whether Option B can replace libobs (DEVELOPMENT.md §16).
//!
//! `recorder/libobs/` stays as the fallback and as a selectable second backend
//! for exactly one release. WS8 deletes it.

// Nothing calls these until #236 wires the backend in, so outside the tests
// they are dead code. Remove each allow as its module gains a caller.
#[cfg_attr(not(test), allow(dead_code))]
pub mod clock;
#[cfg_attr(not(test), allow(dead_code))]
pub mod pcm;
#[cfg_attr(not(test), allow(dead_code))]
pub mod select;

#[cfg(target_os = "windows")]
mod win;
