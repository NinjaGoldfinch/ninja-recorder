//! The own capture backend — WGC → D3D11 → Media Foundation.
//!
//! **Constructible since #236, and only selected by a devtools build.** WS1.6
//! builds it in pieces (the plan is the comment on #10). It records the game
//! window's video and every source the audio preset names (the game by
//! process loopback since #237; the microphone, the desktop and applications
//! such as Discord since #238), each on the video's clock, into one
//! fragmented MP4 with **every track** of the preset's layout (#239): track 0,
//! the mix, and each stem after it. The encoders are Media Foundation
//! transforms driven directly, and the file is written by our own MP4 writer
//! (`crate::mp4::write`), because Media Foundation's sink writer holds one
//! audio stream (DEVELOPMENT.md §2.5).
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
//! - `feed` — one audio source's packets, through its aligner, to the mixer,
//!   never past the video.
//! - `fit` — where a frame from a resized window goes in the fixed-size
//!   output: scaled, aspect kept, centred, black around it.
//! - `mft` — the bookkeeping of an asynchronous (hardware) encoder MFT:
//!   `NeedInput` credits, owed outputs, the bounded frame queue, the drain.
//! - `mix` — one written track: its sources summed in 10 ms blocks on the
//!   aligned timeline, released by a watermark so no source can stall it,
//!   clamped, then i16 for the encoder.
//! - `mux` — encoded samples into the file: created at the first keyframe,
//!   100 ns times to each track's timescale, a fragment per GOP.
//! - `nv12` — BGRA to NV12 on the CPU, BT.709 studio range, for a device with
//!   no video processor.
//! - `pcm` — endpoint sample formats to stereo f32 for the mixer (or i16).
//! - `plan` — which sources a preset's layout opens and what each written
//!   track sums, and the layout that is left once some fail to open.
//! - `problem` — which capture outcomes are failures to tell someone about
//!   (a source there and not capturable, one that stopped part-way, an early
//!   end) and which are absences the preset allows (Discord not running).
//! - `root` — which process tree a process-loopback capture targets: the
//!   game, or the top of an application's tree.
//! - `select` — which H.264 encoder to use (hardware first, the software MFT
//!   only as a marked fallback), the Windows build floor, and the checked
//!   layout of an audio preset.
//! - `stats` — the session summary: one line in the log at start, one at
//!   stop (and one for the repair and the remux), rendered from plain
//!   counters.
//! - `status` — the even frame size, whether the encoder that was activated
//!   is the one `select` chose, and the backend's name.
//! - `worker` — the capture worker (`--capture-worker`, #241): the process
//!   the session runs in, its protocol, its loop, when it exists, and the
//!   daemon's client for it.
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
//!    not whether Option B can replace libobs (DEVELOPMENT.md §16). WASAPI
//!    endpoints for the microphone and the desktop, as `spikes/p0c-video`
//!    captured them.
//!
//! `recorder/libobs/` stays as the fallback and as a selectable second backend
//! for exactly one release. WS8 deletes it.

// Only `own::win` calls these, so outside the tests on a non-Windows host
// they are dead code; and some items in `clock`, `pcm`, `root` and `select`
// have no caller on Windows either (`to_stereo_i16`, which the mixer's f32
// path replaced, and a few the log does not read). Remove each allow once
// every item in its module has a caller.
#[cfg_attr(not(test), allow(dead_code))]
pub mod clock;
#[cfg_attr(not(any(test, target_os = "windows")), allow(dead_code))]
pub mod feed;
#[cfg_attr(not(any(test, target_os = "windows")), allow(dead_code))]
pub mod fit;
#[cfg_attr(not(any(test, target_os = "windows")), allow(dead_code))]
pub mod mft;
#[cfg_attr(not(any(test, target_os = "windows")), allow(dead_code))]
pub mod mix;
#[cfg_attr(not(any(test, target_os = "windows")), allow(dead_code))]
pub mod mux;
#[cfg_attr(not(any(test, target_os = "windows")), allow(dead_code))]
pub mod nv12;
#[cfg_attr(not(test), allow(dead_code))]
pub mod pcm;
#[cfg_attr(not(any(test, target_os = "windows")), allow(dead_code))]
pub mod plan;
#[cfg_attr(not(any(test, target_os = "windows")), allow(dead_code))]
pub mod problem;
#[cfg_attr(not(test), allow(dead_code))]
pub mod root;
#[cfg_attr(not(test), allow(dead_code))]
pub mod select;
#[cfg_attr(not(any(test, target_os = "windows")), allow(dead_code))]
pub mod stats;
#[cfg_attr(not(any(test, target_os = "windows")), allow(dead_code))]
pub mod status;
// Compiled everywhere, like the pure modules: the protocol, the worker's loop
// and the lifetime rule are tested on any host. Off Windows `--capture-worker`
// still runs, and refuses every request with the reason.
pub mod worker;

#[cfg(target_os = "windows")]
mod win;

#[cfg(target_os = "windows")]
pub use win::{OwnRecorder, windows_build};
