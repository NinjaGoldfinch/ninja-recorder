//! A recording's output: the frames, the encoders and the file, together
//! (#239). This is what replaced the sink writer.
//!
//! One [`Output`] per recording, owned by the session thread:
//!
//! - each tick's slot becomes an NV12 frame (`convert.rs`) and goes to the
//!   H.264 encoder (`h264.rs`);
//! - each audio track's PCM goes to that track's own AAC encoder (`aac.rs`);
//! - everything either encoder produces goes to the mux (`own::mux`), which
//!   writes one fragmented MP4 with a track per stem, closing a fragment
//!   before each keyframe.
//!
//! At stop the encoders are drained and the mux writes its `mfra`. A kill
//! at any point leaves every fragment closed before it on disk, whole, which
//! `mp4::write::repair` finishes.

use std::path::Path;

use super::aac::AacEncoder;
use super::audio::SAMPLE_RATE;
use super::capture::Slot;
use super::device::Device;
use super::convert::Frames;
use super::encode::Encoded;
use super::h264::{InputMode, VideoEncoder};
use crate::recorder::own::fit::Size;
use crate::recorder::own::mux::{Mux, MuxStats};
use crate::recorder::own::select;
use crate::recorder::own::status::Loaded;

/// Everything between the slots and the file.
pub struct Output {
    video: VideoEncoder,
    frames: Frames,
    audio: Vec<AacEncoder>,
    mux: Mux,
    /// Encoded samples between an encoder and the mux; empty between calls.
    encoded: Vec<Encoded>,
}

impl Output {
    /// Activates `encoder` for `size` at `fps` and an AAC encoder for each of
    /// `audio_tracks` tracks, into a file at `path` that is created at the
    /// first keyframe. A hardware encoder is asked for texture input, which a
    /// D3D11-aware one gets; `slots` is how many slots frames come from.
    pub fn create(
        path: &Path,
        device: &Device,
        size: Size,
        fps: u32,
        encoder: &select::Encoder,
        slots: usize,
        audio_tracks: usize,
    ) -> Result<Output, String> {
        let (width, height) = (size.width, size.height);
        let video =
            VideoEncoder::create(encoder, &device.device, width, height, fps, encoder.hardware)?;
        let frames = Frames::new(device, size, slots, video.input)?;
        let audio = (0..audio_tracks)
            .map(|_| AacEncoder::create(SAMPLE_RATE))
            .collect::<Result<Vec<_>, _>>()?;
        let mux = Mux::new(path, audio_tracks, SAMPLE_RATE)
            .map_err(|e| format!("the file cannot hold {audio_tracks} AAC track(s): {e}"))?;
        Ok(Output { video, frames, audio, mux, encoded: Vec::new() })
    }

    /// What was activated as the video encoder, for `status::check_loaded`.
    pub fn loaded(&self) -> &Loaded {
        &self.video.loaded
    }

    /// How the video is encoded, for the log line at start: the driving,
    /// the frames, and anything the encoder refused.
    pub fn describe(&self) -> String {
        let driving = if self.video.is_async() {
            "asynchronous, driven by its events"
        } else {
            "synchronous"
        };
        let input = match self.video.input {
            InputMode::Texture => "texture input",
            InputMode::Memory => "system-memory input",
        };
        let mut line = format!("{driving}, {input}, {}", self.frames.describe());
        if !self.video.refused.is_empty() {
            line.push_str(&format!("; the encoder refused: {}", self.video.refused.join("; ")));
        }
        line
    }

    /// Encodes one tick showing `slots[index]`, at `time` for `duration`
    /// (100 ns units).
    pub fn write(
        &mut self,
        device: &Device,
        slots: &[Slot],
        index: usize,
        time: i64,
        duration: i64,
    ) -> Result<(), String> {
        let sample = self.frames.sample(device, slots, index, time, duration)?;
        self.video.encode(sample, &mut self.encoded)?;
        self.mux_video()
    }

    /// Collects what an asynchronous encoder has finished since the last
    /// call. Called on every pass of the cadence loop.
    pub fn poll(&mut self) -> Result<(), String> {
        self.video.poll(&mut self.encoded)?;
        self.mux_video()
    }

    /// Encodes `pcm` (stereo i16) for audio track `track`, starting at sample
    /// `position` of that track.
    pub fn write_audio(&mut self, track: usize, pcm: &[i16], position: u64) -> Result<(), String> {
        let encoder = self.audio.get_mut(track).ok_or_else(|| format!("no audio track {track}"))?;
        encoder.encode(pcm, position, &mut self.encoded)?;
        self.mux_audio(track)
    }

    /// Drains every encoder and closes the file. Consumes the output, so the
    /// encoders are shut down before the caller lets the slots go.
    pub fn finalize(mut self) -> Result<MuxStats, String> {
        let mut problems = Vec::new();
        let video = self.video.drain(&mut self.encoded);
        if let Err(e) = video.and_then(|()| self.mux_video()) {
            problems.push(format!("video: {e}"));
        }
        for track in 0..self.audio.len() {
            let drained = self.audio[track].drain(&mut self.encoded);
            if let Err(e) = drained.and_then(|()| self.mux_audio(track)) {
                problems.push(format!("audio track {track}: {e}"));
            }
        }
        let deepest = self.video.deepest_queue();
        match self.mux.finish() {
            Ok(stats) if problems.is_empty() => Ok(stats),
            Ok(stats) => Err(format!(
                "the file was closed ({} frames), but the end of the encoding failed: {}",
                stats.video_frames,
                problems.join("; ")
            )),
            Err(e) => {
                problems.push(format!("the file: {e}"));
                Err(format!("{} (encoder queue at most {deepest})", problems.join("; ")))
            }
        }
    }

    fn mux_video(&mut self) -> Result<(), String> {
        for sample in self.encoded.drain(..) {
            self.mux
                .video(sample.time, sample.duration, sample.keyframe, sample.bytes)
                .map_err(|e| format!("writing a video frame failed: {e}"))?;
        }
        Ok(())
    }

    fn mux_audio(&mut self, track: usize) -> Result<(), String> {
        for sample in self.encoded.drain(..) {
            self.mux
                .audio(track, sample.time, sample.bytes)
                .map_err(|e| format!("writing an AAC frame to track {track} failed: {e}"))?;
        }
        Ok(())
    }
}
