//! The own capture backend — WGC → D3D11 → Media Foundation.
//!
//! **Constructible since #236, and only selected by a devtools build.** WS1.6
//! builds it in pieces (the plan is the comment on #10). Since #237 it records
//! the game window's video and the game's own audio (process loopback, on the
//! video's clock) into a fragmented MP4 through Media Foundation's sink
//! writer: the **Game** audio preset, and only that one until #238 and #239.
//! The default backend stays libobs until #243, and the Settings row that
//! selects this one stays devtools-only until then (DEVELOPMENT.md §16, "The
//! switch, and when it applies").
//!
//! This is Option B, the *target* backend: it is what removes libobs, and
//! removing libobs is what allows the licence to change at v2.1. It plugs in
//! behind the `Recorder` trait in `recorder/mod.rs` exactly as
//! `recorder/libobs/` and `recorder/stub.rs` do, and nothing above the trait
//! learns which one is linked.
//!
//! Split the way `spikes/p0c-video` is: the pure modules are compiled and
//! unit-tested on every platform, and only `win/` is gated to Windows.
//!
//! - `clock` — the video tick grid, the audio [`clock::Aligner`] that places
//!   packets on it, and whether a packet's stamp is QPC at all.
//! - `feed` — one audio source's packets, through the aligner, to the
//!   encoder, never past the video.
//! - `pcm` — endpoint sample formats to stereo i16 for an encoder, or f32 for
//!   the mixer.
//! - `root` — which process tree a process-loopback capture targets: the
//!   game, or the top of an application's tree.
//! - `select` — which H.264 encoder to use (hardware first, the software MFT
//!   only as a marked fallback), the Windows build floor, and which audio
//!   presets the backend records yet.
//! - `status` — the even frame size, whether the encoder Media Foundation
//!   loaded is the one `select` chose, and the backend's name.
//! - `win` — everything that calls Windows, and `OwnRecorder`.
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

// Only `own::win` calls these, so outside the tests on a non-Windows host
// they are dead code; and parts of `clock`, `pcm` and `root` wait for #238 on
// Windows too (the mixer's f32 path, `application_root`). Remove each allow
// once every item in its module has a caller.
#[cfg_attr(not(test), allow(dead_code))]
pub mod clock;
#[cfg_attr(not(any(test, target_os = "windows")), allow(dead_code))]
pub mod feed;
#[cfg_attr(not(test), allow(dead_code))]
pub mod pcm;
#[cfg_attr(not(test), allow(dead_code))]
pub mod root;
#[cfg_attr(not(test), allow(dead_code))]
pub mod select;
#[cfg_attr(not(any(test, target_os = "windows")), allow(dead_code))]
pub mod status;

#[cfg(target_os = "windows")]
mod win;

#[cfg(target_os = "windows")]
pub use win::{OwnRecorder, windows_build};
