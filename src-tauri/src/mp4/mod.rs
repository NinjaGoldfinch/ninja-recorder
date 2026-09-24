//! MP4 containers, read and written by the app itself rather than by ffmpeg.

pub mod read;

pub use read::{Summary, summarize};
