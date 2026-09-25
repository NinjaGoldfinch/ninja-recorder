//! One audio source's packets, from the capture thread to the mixer, with no
//! Windows in it.
//!
//! Ported from the loop in `spikes/p0c-video/src/win/mod.rs` that fed its
//! sink writer, and pulled out of the session so the whole path, packets in
//! and PCM out at a sample position, is a unit test. Since #238 every source
//! has one [`Feed`], and `mix::Mixdown` hands each a writer into its lane of
//! the mixer, which is what writes to the encoder.
//!
//! Three rules, all the spike's:
//!
//! - **Audio never runs past the video.** A packet waits until the video has
//!   been written past its end, so the audio track can never end after the
//!   video one, and at stop the one straddling the last tick is cut to it.
//! - **Every packet goes through one [`Aligner`]**, which decides what is
//!   dropped, repeated or padded, and whose counters are what the log
//!   reports.
//! - **The track ends where the video does**, padded with silence.
//!
//! And one the spike did not need, because it captured an endpoint with a
//! keep-alive stream: a process-loopback source may deliver nothing while its
//! target is silent, so [`Feed::hold`] writes silence up to the mixer's
//! watermark rather than leave the source's aligner behind the mix.

use std::collections::VecDeque;

use super::clock::{self, Aligner, AudioClock};

/// One packet, stereo f32, as the capture thread sends it.
#[derive(Clone, Debug)]
pub struct Packet {
    /// The time of its first frame on the performance counter, in 100 ns
    /// units: WASAPI's own stamp in [`AudioClock::Qpc`] mode, or
    /// [`clock::DeviceTimeline`]'s in [`AudioClock::Device`] mode.
    pub hns: i64,
    pub frames: u32,
    /// WASAPI's `AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY`, or a re-anchor.
    pub discontinuity: bool,
    /// Interleaved L R, full scale -1.0..1.0 and unclamped (`pcm::to_stereo_f32`):
    /// the mixer clamps once, after summing. Zeros for a packet the engine
    /// marked silent.
    pub pcm: Vec<f32>,
    /// Whose clock `hns` is on, decided by the source at its first packet.
    pub clock: AudioClock,
}

/// Silence goes to the encoder in chunks of at most this many frames, so a
/// long hole is not one enormous buffer.
const SILENCE_CHUNK: u64 = 48_000;

/// Where one source's packets wait, and the aligner that places them.
pub struct Feed {
    aligner: Aligner,
    origin: i64,
    pending: VecDeque<Packet>,
}

impl Feed {
    /// A feed for a source at `rate`, whose tick 0 is `origin` on the
    /// performance counter.
    pub fn new(rate: u32, origin: i64) -> Self {
        Feed { aligner: Aligner::new(rate, AudioClock::Qpc), origin, pending: VecDeque::new() }
    }

    pub fn aligner(&self) -> &Aligner {
        &self.aligner
    }

    /// Queues a packet. Its clock becomes the aligner's.
    pub fn push(&mut self, packet: Packet) {
        self.aligner.set_clock(packet.clock);
        self.pending.push_back(packet);
    }

    /// Writes every queued packet that ends by `video_end_rel`, the end of
    /// the last video tick written, relative to the origin. `write` gets
    /// stereo f32 and the sample position it starts at, and the positions it
    /// sees are contiguous from 0.
    pub fn write_ready<W>(&mut self, video_end_rel: i64, write: &mut W) -> Result<(), String>
    where
        W: FnMut(&[f32], u64) -> Result<(), String>,
    {
        let rate = self.aligner.rate();
        let limit = clock::samples_at(video_end_rel, rate);
        while let Some(packet) = self.pending.front() {
            let end = clock::samples_at(packet.hns - self.origin, rate) + i64::from(packet.frames);
            if end > limit {
                break;
            }
            let packet = self.pending.pop_front().expect("just seen");
            self.place(&packet, write)?;
        }
        Ok(())
    }

    /// Whether the audio already reaches `end_rel`: written, or queued in a
    /// packet that ends at or after it. At stop, the session waits (briefly)
    /// for this before cutting the track, because the packets for the last
    /// video tick are still in flight when the stop arrives.
    pub fn reaches(&self, end_rel: i64) -> bool {
        let rate = self.aligner.rate();
        let end = clock::samples_at(end_rel, rate);
        let queued = self.pending.back().map(|p| {
            clock::samples_at(p.hns - self.origin, rate) + i64::from(p.frames)
        });
        self.aligner.written() as i64 >= end || queued.is_some_and(|q| q >= end)
    }

    /// When nothing is queued, writes silence up to `until_rel`, relative to
    /// the origin. Returns the frames written. The mixer passes its
    /// watermark, which is never past the video.
    pub fn hold<W>(&mut self, until_rel: i64, write: &mut W) -> Result<u64, String>
    where
        W: FnMut(&[f32], u64) -> Result<(), String>,
    {
        if !self.pending.is_empty() {
            return Ok(0);
        }
        let before = self.aligner.written();
        let silence = self.aligner.hold(until_rel);
        write_silence(before, silence, write)?;
        Ok(silence)
    }

    /// Ends the track at `end_rel`: every queued packet that begins before
    /// it, the one straddling it cut short, then silence to exactly `end_rel`.
    /// Returns the aligner's `(pad, overhang)`.
    pub fn finish<W>(&mut self, end_rel: i64, write: &mut W) -> Result<(u64, u64), String>
    where
        W: FnMut(&[f32], u64) -> Result<(), String>,
    {
        let rate = self.aligner.rate();
        let end = clock::samples_at(end_rel, rate);
        while let Some(mut packet) = self.pending.pop_front() {
            let room = end - clock::samples_at(packet.hns - self.origin, rate);
            if room <= 0 {
                continue;
            }
            if room < i64::from(packet.frames) {
                packet.frames = room as u32;
                packet.pcm.truncate(packet.frames as usize * 2);
            }
            self.place(&packet, write)?;
        }
        let before = self.aligner.written();
        let (pad, overhang) = self.aligner.finish(end_rel);
        write_silence(before, pad, write)?;
        Ok((pad, overhang))
    }

    /// Places one packet and writes what the aligner decided.
    fn place<W>(&mut self, packet: &Packet, write: &mut W) -> Result<(), String>
    where
        W: FnMut(&[f32], u64) -> Result<(), String>,
    {
        let before = self.aligner.written();
        let placement =
            self.aligner.place(packet.frames, packet.hns - self.origin, packet.discontinuity);
        write_silence(before, placement.silence, write)?;
        let frames = packet.frames as usize;
        let skip = (placement.skip as usize).min(frames);
        if skip >= frames {
            return Ok(());
        }
        // A short `pcm` is read as silence rather than past, as `pcm` does.
        let mut pcm = Vec::with_capacity((frames - skip + placement.repeat as usize) * 2);
        let sample = |i: usize| packet.pcm.get(i).copied().unwrap_or(0.0);
        for _ in 0..placement.repeat {
            pcm.extend([sample(skip * 2), sample(skip * 2 + 1)]);
        }
        for i in skip * 2..frames * 2 {
            pcm.push(sample(i));
        }
        write(&pcm, before + placement.silence)
    }
}

/// `frames` of silence from sample `from`, in chunks.
fn write_silence<W>(from: u64, frames: u64, write: &mut W) -> Result<(), String>
where
    W: FnMut(&[f32], u64) -> Result<(), String>,
{
    let mut position = from;
    let mut left = frames;
    while left > 0 {
        let n = left.min(SILENCE_CHUNK);
        write(&vec![0.0f32; n as usize * 2], position)?;
        position += n;
        left -= n;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recorder::own::clock::{DeviceTimeline, HNS_PER_SECOND};

    const RATE: u32 = 48_000;
    const PACKET: u32 = 480;
    const ORIGIN: i64 = 100 * HNS_PER_SECOND;

    fn hns_of(samples: i64) -> i64 {
        samples * HNS_PER_SECOND / i64::from(RATE)
    }

    /// Everything written, checked for contiguity as it arrives.
    #[derive(Default)]
    struct Track {
        samples: Vec<f32>,
    }

    impl Track {
        fn frames(&self) -> u64 {
            self.samples.len() as u64 / 2
        }
        fn writer(&mut self) -> impl FnMut(&[f32], u64) -> Result<(), String> + '_ {
            move |pcm, position| {
                assert_eq!(position, self.frames(), "writes must be contiguous");
                assert_eq!(pcm.len() % 2, 0);
                self.samples.extend_from_slice(pcm);
                Ok(())
            }
        }
    }

    /// A packet whose every sample is `value`, stamped at `samples` after the origin.
    fn packet(samples: i64, value: f32, clock: AudioClock) -> Packet {
        Packet {
            hns: ORIGIN + hns_of(samples),
            frames: PACKET,
            discontinuity: false,
            pcm: vec![value; PACKET as usize * 2],
            clock,
        }
    }

    #[test]
    fn packets_wait_for_the_video_and_the_track_ends_with_it() {
        let mut feed = Feed::new(RATE, ORIGIN);
        let mut track = Track::default();
        for i in 0..10 {
            feed.push(packet(i * 480, (1 + i) as f32, AudioClock::Qpc));
        }
        // Video written to 25 ms: two whole packets fit, the third waits.
        feed.write_ready(hns_of(1_200), &mut track.writer()).unwrap();
        assert_eq!(track.frames(), 960);
        assert!(feed.reaches(hns_of(4_800)), "queued to 100 ms");
        assert!(!feed.reaches(hns_of(4_801)));
        // Stopped at 42 ms: the fifth packet is cut at it, the rest dropped.
        let (pad, overhang) = feed.finish(hns_of(2_016), &mut track.writer()).unwrap();
        assert_eq!((pad, overhang), (0, 0));
        assert_eq!(track.frames(), 2_016);
        assert_eq!(track.samples[2 * 1_920], 5.0);
        assert_eq!(*track.samples.last().unwrap(), 5.0);
    }

    #[test]
    fn a_quiet_source_is_held_behind_the_video_and_resumes_in_place() {
        let mut feed = Feed::new(RATE, ORIGIN);
        let mut track = Track::default();
        feed.push(packet(0, 7.0, AudioClock::Qpc));
        feed.write_ready(hns_of(480), &mut track.writer()).unwrap();
        // Two seconds with no packets; held to a point half a second behind.
        let held = feed.hold(hns_of(72_000), &mut track.writer()).unwrap();
        assert_eq!(track.frames(), 72_000);
        assert_eq!(held, 72_000 - 480);
        // The source resumes with a packet from just now: the gap between
        // the hold and it is silence, and it lands where it belongs.
        feed.push(packet(96_000, 9.0, AudioClock::Qpc));
        feed.write_ready(hns_of(96_480), &mut track.writer()).unwrap();
        assert_eq!(track.frames(), 96_480);
        assert_eq!(track.samples[2 * 96_000], 9.0);
        assert_eq!(track.samples[2 * 95_999], 0.0);
        assert_eq!(feed.aligner().stats.residual_last, 0);
        // Nothing is held while a packet waits.
        feed.push(packet(96_480, 9.0, AudioClock::Qpc));
        assert_eq!(feed.hold(hns_of(200_000), &mut track.writer()).unwrap(), 0);
    }

    #[test]
    fn audio_that_starts_late_is_led_by_silence_and_padded_to_the_end() {
        let mut feed = Feed::new(RATE, ORIGIN);
        let mut track = Track::default();
        feed.push(packet(4_800, 3.0, AudioClock::Qpc));
        feed.write_ready(hns_of(10_000), &mut track.writer()).unwrap();
        let (pad, _) = feed.finish(hns_of(10_000), &mut track.writer()).unwrap();
        assert_eq!(track.frames(), 10_000);
        assert_eq!(pad, 10_000 - 5_280);
        assert_eq!(track.samples[2 * 4_799], 0.0);
        assert_eq!(track.samples[2 * 4_800], 3.0);
        assert_eq!(feed.aligner().stats.lead_silence, 4_800);
    }

    #[test]
    fn a_slip_repeats_the_first_kept_frame() {
        let mut feed = Feed::new(RATE, ORIGIN);
        let mut track = Track::default();
        feed.push(packet(0, 1.0, AudioClock::Qpc));
        // 1 ms late, which is past the slip threshold: one frame repeated.
        let mut late = packet(480 + 48, 2.0, AudioClock::Qpc);
        late.pcm[0] = 5.0;
        late.pcm[1] = 6.0;
        feed.push(late);
        feed.write_ready(hns_of(2_000), &mut track.writer()).unwrap();
        assert_eq!(track.frames(), 961);
        assert_eq!(&track.samples[960..964], &[5.0, 6.0, 5.0, 6.0]);
        assert_eq!(feed.aligner().stats.slips_repeated, 1);
    }

    #[test]
    fn device_time_through_the_feed_is_one_unbroken_track() {
        // Ten seconds of a source whose stamps were no use: the timeline
        // stamps from the count, and the aligner never slips.
        let mut feed = Feed::new(RATE, ORIGIN);
        let mut timeline = DeviceTimeline::new(RATE);
        let mut track = Track::default();
        for i in 0..1_000i64 {
            let arrival = ORIGIN + hns_of(480 * (i + 1)) + (i % 5) * 10_000;
            let (hns, discontinuity) = timeline.stamp(PACKET, arrival);
            feed.push(Packet {
                hns,
                frames: PACKET,
                discontinuity,
                pcm: vec![1.0; PACKET as usize * 2],
                clock: AudioClock::Device,
            });
            feed.write_ready(hns_of(480 * (i + 1)), &mut track.writer()).unwrap();
        }
        feed.finish(10 * HNS_PER_SECOND, &mut track.writer()).unwrap();
        assert_eq!(track.frames(), 480_000);
        assert!(track.samples.iter().all(|&s| s == 1.0));
        let stats = &feed.aligner().stats;
        assert_eq!(stats.slips_dropped + stats.slips_repeated + stats.gaps, 0);
        assert_eq!(feed.aligner().clock(), AudioClock::Device);
    }

    #[test]
    fn a_short_packet_body_is_read_as_silence() {
        let mut feed = Feed::new(RATE, ORIGIN);
        let mut track = Track::default();
        let mut short = packet(0, 4.0, AudioClock::Qpc);
        short.pcm.truncate(10);
        feed.push(short);
        feed.write_ready(hns_of(480), &mut track.writer()).unwrap();
        assert_eq!(track.frames(), 480);
        assert_eq!(track.samples[9], 4.0);
        assert_eq!(track.samples[10], 0.0);
    }
}
