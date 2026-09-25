//! Encoded samples into one file: the H.264 track and one AAC track per
//! written audio track, through `mp4::write::Writer`, with no Windows in it
//! (#239).
//!
//! The encoders (`own/win/h264.rs`, `own/win/aac.rs`) hand over what Media
//! Foundation gives them: an Annex B access unit or a raw AAC frame, stamped
//! in 100 ns units. Everything between that and the writer is decided here,
//! so it is a unit test:
//!
//! - **The file is created at the first keyframe**, because the writer's
//!   `moov` carries the SPS and PPS, and the first keyframe is where every
//!   encoder puts them (`own/win/h264.rs` supplies them from
//!   `MF_MT_MPEG_SEQUENCE_HEADER` if an encoder does not). An AAC frame that
//!   comes first waits for it; a video frame before any keyframe cannot be
//!   decoded and is dropped, and counted.
//! - **Timestamps.** Video goes from 100 ns to the writer's 90 kHz, rounded
//!   to the nearest tick, and each frame's duration is the gap to the next
//!   frame, so the track's decode times are exactly the encoder's
//!   presentation times whatever frames it may have skipped (it emits no
//!   B-frames, so the two orders are the same). That means holding one frame
//!   back until the next arrives. Audio goes in its sample rate: the first
//!   frame at its own time, then every frame 1024 samples after the last,
//!   which is what an AAC-LC frame is, so rounding never accumulates.
//! - **One fragment per GOP.** Immediately before each video keyframe after
//!   the first, the fragment is closed ([`flush_before`]), so every fragment
//!   opens on a keyframe and a killed recording loses at most one GOP, two
//!   seconds. A fragment is also closed after [`MAX_FRAGMENT_FRAMES`]
//!   frames whether or not a keyframe has come, for an encoder that ignores
//!   the GOP it was given.

use std::io;
use std::path::{Path, PathBuf};

use crate::mp4::write::{Track, VIDEO_TIMESCALE, Writer};
use crate::recorder::own::clock::HNS_PER_SECOND;

/// Samples in one AAC-LC frame.
pub const AAC_FRAME: u32 = 1024;

/// The most video frames one fragment holds, keyframe or not: five GOPs.
pub const MAX_FRAGMENT_FRAMES: u32 = 600;

/// How many AAC frames may wait for the first keyframe: ten seconds of
/// 48 kHz for every track. Only an encoder that never produces a keyframe
/// reaches it.
const MAX_EARLY_AUDIO: usize = 470 * 4;

/// 100 ns units to ticks of `timescale`, to the nearest tick. A time before
/// zero is zero: nothing is written before the origin.
pub fn to_timescale(hns: i64, timescale: u32) -> u64 {
    if hns <= 0 {
        return 0;
    }
    let scaled = i128::from(hns) * i128::from(timescale);
    ((scaled + i128::from(HNS_PER_SECOND / 2)) / i128::from(HNS_PER_SECOND)) as u64
}

/// Whether the fragment is closed before a video frame: before every
/// keyframe but the file's first, and after [`MAX_FRAGMENT_FRAMES`] frames
/// regardless. `in_fragment` is the video frames already in the fragment.
pub fn flush_before(keyframe: bool, in_fragment: u32) -> bool {
    in_fragment > 0 && (keyframe || in_fragment >= MAX_FRAGMENT_FRAMES)
}

/// What the mux did, for the log line at stop.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MuxStats {
    pub video_frames: u64,
    pub keyframes: u64,
    /// Frames before the first keyframe, which nothing could decode.
    pub dropped_before_keyframe: u64,
    /// AAC frames written, per audio track.
    pub audio_frames: Vec<u64>,
    /// Fragments closed, the last one by `finish`.
    pub fragments: u64,
}

struct PendingVideo {
    pts: u64,
    /// The encoder's own duration, used only for the last frame.
    duration: u32,
    keyframe: bool,
    bytes: Vec<u8>,
}

/// One recording's file, from the first encoded sample to `finish`.
pub struct Mux {
    path: PathBuf,
    audio: Vec<Track>,
    writer: Option<Writer>,
    /// AAC frames that came before the file existed: (track, pts, bytes).
    early_audio: Vec<(usize, u64, Vec<u8>)>,
    video: Option<PendingVideo>,
    in_fragment: u32,
    /// The next AAC frame's time, per track, once its first has come.
    audio_next: Vec<Option<u64>>,
    pub stats: MuxStats,
}

impl Mux {
    /// A file at `path` with `audio_tracks` stereo AAC-LC tracks at
    /// `sample_rate` after the video. Nothing is written until the first
    /// keyframe.
    pub fn new(path: &Path, audio_tracks: usize, sample_rate: u32) -> io::Result<Mux> {
        let audio = (0..audio_tracks)
            .map(|_| Track::aac_lc(sample_rate, 2))
            .collect::<io::Result<Vec<Track>>>()?;
        Ok(Mux {
            path: path.to_path_buf(),
            audio,
            writer: None,
            early_audio: Vec::new(),
            video: None,
            in_fragment: 0,
            audio_next: vec![None; audio_tracks],
            stats: MuxStats { audio_frames: vec![0; audio_tracks], ..MuxStats::default() },
        })
    }

    /// Whether the file exists yet: the first keyframe has come.
    pub fn is_open(&self) -> bool {
        self.writer.is_some()
    }

    /// One encoded video frame, Annex B, at `time_hns` for `duration_hns`.
    pub fn video(
        &mut self,
        time_hns: i64,
        duration_hns: i64,
        keyframe: bool,
        bytes: Vec<u8>,
    ) -> io::Result<()> {
        if self.writer.is_none() {
            if !keyframe {
                self.stats.dropped_before_keyframe += 1;
                return Ok(());
            }
            self.open(&bytes)?;
        }
        let pts = to_timescale(time_hns, VIDEO_TIMESCALE);
        let duration = to_timescale(duration_hns, VIDEO_TIMESCALE).clamp(1, u64::from(u32::MAX));
        if let Some(previous) = self.video.take() {
            // The gap to this frame; a repeated time keeps the encoder's own.
            let gap = pts.saturating_sub(previous.pts);
            let duration = if gap == 0 { previous.duration } else { gap.min(u64::from(u32::MAX)) as u32 };
            self.write_video(previous, duration)?;
        }
        if flush_before(keyframe, self.in_fragment) {
            self.flush()?;
        }
        self.video = Some(PendingVideo { pts, duration: duration as u32, keyframe, bytes });
        Ok(())
    }

    /// One raw AAC frame for audio track `track` (0 is the file's first audio
    /// track, the mix), which the encoder stamped `time_hns`.
    pub fn audio(&mut self, track: usize, time_hns: i64, bytes: Vec<u8>) -> io::Result<()> {
        let Some(next) = self.audio_next.get_mut(track) else {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("no audio track {track}")));
        };
        let rate = self.audio[track].timescale();
        let pts = next.unwrap_or_else(|| to_timescale(time_hns, rate));
        *next = Some(pts + u64::from(AAC_FRAME));
        match self.writer.as_mut() {
            Some(writer) => {
                writer.write_sample(track + 1, pts, AAC_FRAME, true, &bytes)?;
                self.stats.audio_frames[track] += 1;
            }
            None if self.early_audio.len() < MAX_EARLY_AUDIO => {
                self.early_audio.push((track, pts, bytes));
            }
            None => {
                return Err(io::Error::other(
                    "the video encoder has produced no keyframe, and the audio waiting for it \
                     has reached ten seconds",
                ));
            }
        }
        Ok(())
    }

    /// Writes the last frame and closes the file with its `mfra`. An error if
    /// no keyframe ever came, in which case there is no file.
    pub fn finish(mut self) -> io::Result<MuxStats> {
        if self.writer.is_none() {
            return Err(io::Error::other(format!(
                "no keyframe was encoded ({} frame(s) before one), so no file was written",
                self.stats.dropped_before_keyframe
            )));
        }
        if let Some(last) = self.video.take() {
            let duration = last.duration;
            self.write_video(last, duration)?;
        }
        let writer = self.writer.take().expect("checked above");
        writer.finish()?;
        if self.in_fragment > 0 || self.stats.audio_frames.iter().any(|&n| n > 0) {
            self.stats.fragments += 1;
        }
        Ok(self.stats)
    }

    /// Creates the file from the first keyframe, and writes the audio that
    /// was waiting for it.
    fn open(&mut self, keyframe: &[u8]) -> io::Result<()> {
        let video = Track::h264_from_annex_b(keyframe)?;
        let mut tracks = vec![video];
        tracks.extend(self.audio.iter().cloned());
        let mut writer = Writer::create(&self.path, tracks)?;
        for (track, pts, bytes) in self.early_audio.drain(..) {
            writer.write_sample(track + 1, pts, AAC_FRAME, true, &bytes)?;
            self.stats.audio_frames[track] += 1;
        }
        self.writer = Some(writer);
        Ok(())
    }

    fn write_video(&mut self, frame: PendingVideo, duration: u32) -> io::Result<()> {
        let writer = self.writer.as_mut().ok_or_else(|| io::Error::other("no file yet"))?;
        writer.write_sample(0, frame.pts, duration.max(1), frame.keyframe, &frame.bytes)?;
        self.in_fragment += 1;
        self.stats.video_frames += 1;
        self.stats.keyframes += u64::from(frame.keyframe);
        Ok(())
    }

    fn flush(&mut self) -> io::Result<()> {
        if let Some(writer) = self.writer.as_mut() {
            writer.flush_fragment()?;
            self.stats.fragments += 1;
        }
        self.in_fragment = 0;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mp4;

    /// libx264's SPS for 320x240 Constrained Baseline@2.1 and a PPS, as the
    /// writer's own tests use them.
    const SPS: [u8; 23] = [
        0x67, 0x42, 0xc0, 0x15, 0xd9, 0x01, 0x41, 0xfb, 0x01, 0x10, 0x00, 0x00, 0x03, 0x00, 0x10,
        0x00, 0x00, 0x07, 0x80, 0xf1, 0x62, 0xe4, 0x80,
    ];
    const PPS: [u8; 4] = [0x68, 0xce, 0x38, 0x80];

    /// A fake access unit: SPS and PPS in-band on a keyframe, as encoders
    /// emit them. Box structure only; nothing decodes it.
    fn access_unit(keyframe: bool, n: u8) -> Vec<u8> {
        let mut au = Vec::new();
        if keyframe {
            for nal in [&SPS[..], &PPS[..]] {
                au.extend_from_slice(&[0, 0, 0, 1]);
                au.extend_from_slice(nal);
            }
        }
        au.extend_from_slice(&[0, 0, 0, 1, if keyframe { 0x65 } else { 0x41 }, 0x88, n, 1]);
        au
    }

    fn tick(k: i64) -> i64 {
        crate::recorder::own::clock::tick_time(k as u64, 60)
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nr-own-mux-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(format!("{name}.mp4"))
    }

    #[test]
    fn times_round_to_the_nearest_tick_of_the_timescale() {
        // 60 fps on the tick grid is 1500 ticks of 90 kHz every frame, even
        // though a tick is not a whole number of 100 ns units.
        for k in 0..600 {
            assert_eq!(to_timescale(tick(k), VIDEO_TIMESCALE), k as u64 * 1500, "tick {k}");
        }
        assert_eq!(to_timescale(HNS_PER_SECOND, 48_000), 48_000);
        // An AAC frame at 48 kHz is 213 333.3 units of 100 ns.
        assert_eq!(to_timescale(213_333, 48_000), 1024);
        assert_eq!(to_timescale(-5, 90_000), 0);
        assert_eq!(to_timescale(0, 90_000), 0);
        // Hours in, without overflow.
        assert_eq!(to_timescale(10 * 3600 * HNS_PER_SECOND, VIDEO_TIMESCALE), 10 * 3600 * 90_000);
    }

    #[test]
    fn a_fragment_closes_before_every_keyframe_but_the_first() {
        assert!(!flush_before(true, 0), "the file's first keyframe opens the first fragment");
        assert!(flush_before(true, 1));
        assert!(flush_before(true, 119));
        assert!(!flush_before(false, 1));
        assert!(!flush_before(false, MAX_FRAGMENT_FRAMES - 1));
        // An encoder that ignores the GOP still gets fragments.
        assert!(flush_before(false, MAX_FRAGMENT_FRAMES));
    }

    /// Four seconds at 60 fps with a keyframe every 120 frames, and three
    /// AAC tracks: two fragments and a third for the tail, each opening on a
    /// keyframe, an `mfra`, and every track declared.
    #[test]
    fn a_gop_per_fragment_and_every_track_in_one_file() {
        let path = scratch("gop");
        let mut mux = Mux::new(&path, 3, 48_000).unwrap();
        let mut audio_hns = 0i64;
        let frame_hns = i64::from(AAC_FRAME) * HNS_PER_SECOND / 48_000;
        for k in 0..250 {
            let key = k % 120 == 0;
            mux.video(tick(k), tick(k + 1) - tick(k), key, access_unit(key, k as u8)).unwrap();
            while audio_hns < tick(k + 1) {
                for track in 0..3 {
                    mux.audio(track, audio_hns, vec![0x21, track as u8, 3]).unwrap();
                }
                audio_hns += frame_hns;
            }
        }
        let stats = mux.finish().unwrap();
        assert_eq!(stats.video_frames, 250);
        assert_eq!(stats.keyframes, 3);
        assert_eq!(stats.fragments, 3);
        assert_eq!(stats.dropped_before_keyframe, 0);
        assert!(stats.audio_frames.iter().all(|&n| n == stats.audio_frames[0] && n > 0));

        let summary = mp4::summarize(&mut std::fs::File::open(&path).unwrap()).unwrap();
        assert_eq!(summary.tracks, 4, "{summary:?}");
        assert_eq!(summary.audio_tracks, 3, "{summary:?}");
        assert_eq!(summary.complete_fragments, 3, "{summary:?}");
        assert!(summary.mfra && summary.structurally_playable(), "{summary:?}");
        assert_eq!(summary.truncated, None);
        // A finished file needs no repair.
        assert!(mp4::write::repair(&path).unwrap().already_complete);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// Audio encoded before the first keyframe waits for the file, and video
    /// before it is dropped, since nothing could decode it.
    #[test]
    fn nothing_is_written_before_the_first_keyframe() {
        let path = scratch("early");
        let mut mux = Mux::new(&path, 1, 48_000).unwrap();
        mux.audio(0, 0, vec![1, 2, 3]).unwrap();
        mux.audio(0, 213_333, vec![1, 2, 3]).unwrap();
        mux.video(tick(0), tick(1), false, access_unit(false, 0)).unwrap();
        assert!(!mux.is_open());
        assert!(!path.exists(), "no file before a keyframe");
        mux.video(tick(1), tick(2) - tick(1), true, access_unit(true, 1)).unwrap();
        assert!(mux.is_open() && path.exists());
        mux.video(tick(2), tick(3) - tick(2), false, access_unit(false, 2)).unwrap();
        let stats = mux.finish().unwrap();
        assert_eq!(stats.dropped_before_keyframe, 1);
        assert_eq!(stats.video_frames, 2);
        assert_eq!(stats.audio_frames, vec![2]);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn no_keyframe_at_all_is_an_error_and_no_file() {
        let path = scratch("none");
        let mut mux = Mux::new(&path, 2, 48_000).unwrap();
        mux.video(0, tick(1), false, access_unit(false, 0)).unwrap();
        let e = mux.finish().unwrap_err();
        assert!(e.to_string().contains("no keyframe"), "{e}");
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_track_past_the_layout_is_refused() {
        let path = scratch("past");
        let mut mux = Mux::new(&path, 2, 48_000).unwrap();
        assert!(mux.audio(2, 0, vec![1]).is_err());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// A video-only recording (no source opened) is a one-track file.
    #[test]
    fn no_audio_tracks_is_a_video_only_file() {
        let path = scratch("video-only");
        let mut mux = Mux::new(&path, 0, 48_000).unwrap();
        for k in 0..10 {
            mux.video(tick(k), tick(k + 1) - tick(k), k == 0, access_unit(k == 0, k as u8)).unwrap();
        }
        mux.finish().unwrap();
        let summary = mp4::summarize(&mut std::fs::File::open(&path).unwrap()).unwrap();
        assert_eq!((summary.tracks, summary.audio_tracks), (1, 0), "{summary:?}");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// Killed after two fragments: the file on disk is whole up to the last
    /// flush, and `repair` makes it a finished file with every track.
    #[test]
    fn a_killed_recording_is_whole_up_to_its_last_gop_and_repairs() {
        let path = scratch("killed");
        let mut mux = Mux::new(&path, 2, 48_000).unwrap();
        for k in 0..300 {
            let key = k % 120 == 0;
            mux.video(tick(k), tick(k + 1) - tick(k), key, access_unit(key, k as u8)).unwrap();
            if k % 4 == 0 {
                mux.audio(0, tick(k), vec![9]).unwrap();
                mux.audio(1, tick(k), vec![8]).unwrap();
            }
        }
        drop(mux);
        let summary = mp4::summarize(&mut std::fs::File::open(&path).unwrap()).unwrap();
        assert_eq!(summary.complete_fragments, 2, "{summary:?}");
        assert!(!summary.mfra);
        let repaired = mp4::write::repair(&path).unwrap();
        assert_eq!(repaired.fragments, 2);
        let summary = mp4::summarize(&mut std::fs::File::open(&path).unwrap()).unwrap();
        assert!(summary.mfra && summary.structurally_playable(), "{summary:?}");
        assert_eq!((summary.tracks, summary.audio_tracks), (3, 2));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
