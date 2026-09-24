//! ISO base media files (MP4), in pure Rust with no Windows calls.
//!
//! `write` is the own backend's muxer (WS1.6.3, #235): Media Foundation's MP4
//! sinks hold one audio stream, and every preset except Game needs 2-4
//! tracks. See DEVELOPMENT.md §2.5.

pub mod write;
