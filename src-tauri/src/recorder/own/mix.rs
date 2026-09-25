//! Track 0: every source a preset names, summed on the video's timeline, with
//! no Windows in it (#238).
//!
//! The libobs backend hands each source to libobs' mixer. The own backend
//! captures each source itself (DEVELOPMENT.md §2.5, "The own backend
//! captures each source itself"), so it mixes them itself too, and does it
//! here so the whole of it is a unit test:
//!
//! - **Every source is aligned before it is mixed.** A source's packets go
//!   through its own [`Feed`] and so its own `clock::Aligner`, which places
//!   them on the sample timeline laid from the video's origin (slipping frames
//!   on QPC, or trusting the device count). What reaches the [`Mixer`] is one
//!   contiguous stream per source, and sample `n` of every stream is the same
//!   instant.
//! - **The mix is made in fixed 10 ms blocks** on that timeline. A block is
//!   mixed as soon as every source has delivered it, and in any case once the
//!   watermark, [`LATENCY`] behind now, has passed its end. A source with
//!   nothing for a block contributes silence: the game while it is quiet
//!   (process loopback may send nothing at all), Discord's tree while nobody
//!   speaks, a microphone that has been unplugged. **No source can stall the
//!   mix**, and what arrives for a block already mixed is dropped and counted.
//! - **Sum, clamp to [-1, 1], then i16**, once, for the encoder. The sources
//!   are carried as f32 until then, so quantising happens once and a sum that
//!   clips is clipped, not wrapped.
//! - **#237's guarantees hold for the mix**: nothing is mixed past the end of
//!   the video written so far, and at stop every source is padded, and the
//!   mix written, to the last tick exactly.

use std::collections::VecDeque;

use super::clock::{self, HNS_PER_SECOND};
use super::feed::{Feed, Packet};
use super::pcm;

/// Blocks per second: a block is 10 ms, 480 frames at 48 kHz.
pub const BLOCKS_PER_SECOND: u32 = 100;

/// How far behind now the watermark sits, 150 ms: a block whose end is
/// further back than this is mixed whether or not every source has delivered
/// it. Far more than a shared-mode packet's delivery (a period or two of
/// 10 ms), and short enough that the encoder is never left long without
/// audio. #237 held a quiet game half a second behind; with several sources
/// the mix waits on the slowest, so the margin is tighter.
pub const LATENCY: i64 = HNS_PER_SECOND * 15 / 100;

/// What the mixer did, for the log line at stop.
#[derive(Clone, Debug, Default)]
pub struct MixStats {
    /// Blocks written, the last of them possibly short.
    pub blocks: u64,
    /// Blocks the watermark released with at least one source short of them.
    pub released: u64,
    /// Output samples (left and right counted apart) the sum took past full
    /// scale, clamped.
    pub clipped: u64,
}

/// One source's samples waiting to be mixed.
#[derive(Default)]
struct Lane {
    /// Stereo interleaved, starting at the mixer's `emitted` frame.
    pending: VecDeque<f32>,
    /// One past the last frame this source has been given.
    end: u64,
    /// Frames that came for blocks already mixed, dropped.
    late: u64,
    /// Frames this source had nothing for when its block was mixed.
    missing: u64,
}

/// Sums any number of aligned sources into one stereo i16 stream in fixed
/// blocks. Sources are indexed from 0; positions are frames from the video's
/// origin.
pub struct Mixer {
    rate: u32,
    block: u64,
    lanes: Vec<Lane>,
    /// Frames mixed and written so far: the next block starts here.
    emitted: u64,
    pub stats: MixStats,
}

impl Mixer {
    /// A mixer for `sources` sources at `rate`.
    pub fn new(rate: u32, sources: usize) -> Self {
        Mixer {
            rate,
            block: u64::from((rate / BLOCKS_PER_SECOND).max(1)),
            lanes: (0..sources).map(|_| Lane::default()).collect(),
            emitted: 0,
            stats: MixStats::default(),
        }
    }

    /// Frames written so far.
    pub fn emitted(&self) -> u64 {
        self.emitted
    }

    /// Frames of source `lane` that arrived after their block was mixed.
    pub fn late(&self, lane: usize) -> u64 {
        self.lanes.get(lane).map_or(0, |l| l.late)
    }

    /// Frames source `lane` contributed silence for because it had nothing.
    pub fn missing(&self, lane: usize) -> u64 {
        self.lanes.get(lane).map_or(0, |l| l.missing)
    }

    /// Gives source `lane` the stereo samples `pcm`, starting at frame
    /// `position`. A [`Feed`] only ever writes contiguously from 0; anything
    /// else is still handled, a hole as silence and a repeat as dropped. What
    /// falls before the next block to be mixed is late and dropped.
    pub fn push(&mut self, lane: usize, pcm: &[f32], position: u64) {
        let emitted = self.emitted;
        let Some(l) = self.lanes.get_mut(lane) else {
            return;
        };
        let frames = (pcm.len() / 2) as u64;
        let end = position + frames;
        if end <= l.end {
            return;
        }
        // The lane holds [emitted, max(l.end, emitted)); a hole up to
        // `position` is silence, and anything before `emitted` is late.
        let hole_from = l.end.max(emitted);
        let hole_to = position.max(emitted);
        if hole_to > hole_from {
            l.pending.extend(std::iter::repeat_n(0.0, ((hole_to - hole_from) * 2) as usize));
        }
        let fresh = position.max(l.end);
        let keep_from = fresh.max(emitted);
        l.late += keep_from.min(end) - fresh;
        if end > keep_from {
            l.pending.extend(&pcm[((keep_from - position) * 2) as usize..(frames * 2) as usize]);
        }
        l.end = end;
    }

    /// Writes every whole block that ends by `limit_rel`, the end of the video
    /// written so far, and that either every source has delivered or the
    /// watermark `watermark_rel` has passed. Both are relative to the origin.
    /// Returns the frames written.
    pub fn mix<W>(&mut self, limit_rel: i64, watermark_rel: i64, write: &mut W) -> Result<u64, String>
    where
        W: FnMut(&[i16], u64) -> Result<(), String>,
    {
        let limit = frames_at(limit_rel, self.rate);
        let watermark = frames_at(watermark_rel, self.rate);
        let before = self.emitted;
        loop {
            let block_end = self.emitted + self.block;
            if block_end > limit {
                break;
            }
            let delivered = self.lanes.iter().all(|l| l.end >= block_end);
            if !delivered {
                if block_end > watermark {
                    break;
                }
                self.stats.released += 1;
            }
            self.emit(self.block, write)?;
        }
        Ok(self.emitted - before)
    }

    /// Writes everything up to `end_rel` whatever the sources have delivered,
    /// the last block cut short to land on it exactly. Returns the frames
    /// written.
    pub fn finish<W>(&mut self, end_rel: i64, write: &mut W) -> Result<u64, String>
    where
        W: FnMut(&[i16], u64) -> Result<(), String>,
    {
        let end = frames_at(end_rel, self.rate);
        let before = self.emitted;
        while self.emitted < end {
            let n = self.block.min(end - self.emitted);
            self.emit(n, write)?;
        }
        Ok(self.emitted - before)
    }

    /// Mixes and writes the next `n` frames.
    fn emit<W>(&mut self, n: u64, write: &mut W) -> Result<(), String>
    where
        W: FnMut(&[i16], u64) -> Result<(), String>,
    {
        let samples = (n * 2) as usize;
        let mut sum = vec![0.0f32; samples];
        for lane in &mut self.lanes {
            let have = lane.pending.len().min(samples);
            for (out, v) in sum.iter_mut().zip(lane.pending.drain(..have)) {
                *out += v;
            }
            lane.missing += ((samples - have) / 2) as u64;
        }
        self.stats.clipped += sum.iter().filter(|v| v.abs() > 1.0).count() as u64;
        write(&pcm::f32_to_i16(&sum), self.emitted)?;
        self.emitted += n;
        self.stats.blocks += 1;
        Ok(())
    }
}

/// Frames at `rate` from the origin to `rel_hns`, never negative.
fn frames_at(rel_hns: i64, rate: u32) -> u64 {
    clock::samples_at(rel_hns, rate).max(0) as u64
}

/// Every source of a recording, each through its own [`Feed`], into one
/// [`Mixer`]: what the session thread drives on each pass of its loop, and
/// what the Windows test drives with synthetic packets.
pub struct Mixdown {
    feeds: Vec<Feed>,
    mixer: Mixer,
}

impl Mixdown {
    /// `sources` sources at `rate`, whose tick 0 is `origin` on the
    /// performance counter.
    pub fn new(rate: u32, origin: i64, sources: usize) -> Self {
        Mixdown {
            feeds: (0..sources).map(|_| Feed::new(rate, origin)).collect(),
            mixer: Mixer::new(rate, sources),
        }
    }

    /// Queues a packet from source `source`.
    pub fn push(&mut self, source: usize, packet: Packet) {
        if let Some(feed) = self.feeds.get_mut(source) {
            feed.push(packet);
        }
    }

    pub fn feed(&self, source: usize) -> &Feed {
        &self.feeds[source]
    }

    pub fn mixer(&self) -> &Mixer {
        &self.mixer
    }

    /// One pass: every source's packets that the video has passed go into its
    /// lane, a source with nothing queued is held with silence up to the
    /// watermark, and every block that is ready is mixed and written.
    /// `video_end_rel` is the end of the last video tick written and `now_rel`
    /// the performance counter now, both relative to the origin.
    ///
    /// The watermark is `now - LATENCY`, but never past the video: a video
    /// loop that has fallen behind holds the audio back with it.
    pub fn write<W>(&mut self, video_end_rel: i64, now_rel: i64, write: &mut W) -> Result<(), String>
    where
        W: FnMut(&[i16], u64) -> Result<(), String>,
    {
        let watermark = (now_rel - LATENCY).min(video_end_rel);
        for (i, feed) in self.feeds.iter_mut().enumerate() {
            let mixer = &mut self.mixer;
            let mut lane = |pcm: &[f32], position: u64| {
                mixer.push(i, pcm, position);
                Ok(())
            };
            feed.write_ready(video_end_rel, &mut lane)?;
            // The mixer would release the blocks anyway; holding the feed
            // too keeps its aligner level with the mix, so a source that
            // resumes is placed from there, and the log counts a quiet
            // stretch as held rather than as one enormous gap.
            feed.hold(watermark, &mut lane)?;
        }
        self.mixer.mix(video_end_rel, watermark, write)?;
        Ok(())
    }

    /// Ends the track at `end_rel`, the end of the last video tick: each
    /// source's queued packets up to it, the one straddling it cut, its lane
    /// padded to it; then the mix, to it exactly. Returns each source's
    /// padding in frames, in source order.
    pub fn finish<W>(&mut self, end_rel: i64, write: &mut W) -> Result<Vec<u64>, String>
    where
        W: FnMut(&[i16], u64) -> Result<(), String>,
    {
        let mut pads = Vec::with_capacity(self.feeds.len());
        for (i, feed) in self.feeds.iter_mut().enumerate() {
            let mixer = &mut self.mixer;
            let mut lane = |pcm: &[f32], position: u64| {
                mixer.push(i, pcm, position);
                Ok(())
            };
            let (pad, _) = feed.finish(end_rel, &mut lane)?;
            pads.push(pad);
        }
        self.mixer.finish(end_rel, write)?;
        Ok(pads)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recorder::own::clock::AudioClock;

    const RATE: u32 = 48_000;
    const BLOCK: u64 = 480;
    const ORIGIN: i64 = 100 * HNS_PER_SECOND;

    fn hns_of(frames: u64) -> i64 {
        frames as i64 * HNS_PER_SECOND / i64::from(RATE)
    }

    /// Everything written, checked for contiguity as it arrives.
    #[derive(Default)]
    struct Out {
        samples: Vec<i16>,
        writes: Vec<usize>,
    }

    impl Out {
        fn frames(&self) -> u64 {
            self.samples.len() as u64 / 2
        }
        fn writer(&mut self) -> impl FnMut(&[i16], u64) -> Result<(), String> + '_ {
            move |pcm, position| {
                assert_eq!(position, self.frames(), "writes must be contiguous");
                self.writes.push(pcm.len() / 2);
                self.samples.extend_from_slice(pcm);
                Ok(())
            }
        }
        /// The left sample of frame `n`.
        fn at(&self, n: u64) -> i16 {
            self.samples[(n * 2) as usize]
        }
    }

    fn level(v: f32) -> i16 {
        pcm::f32_to_i16(&[v])[0]
    }

    fn constant(frames: u64, value: f32) -> Vec<f32> {
        vec![value; (frames * 2) as usize]
    }

    #[test]
    fn two_sources_are_summed_block_by_block() {
        let mut m = Mixer::new(RATE, 2);
        let mut out = Out::default();
        m.push(0, &constant(960, 0.25), 0);
        m.push(1, &constant(960, 0.125), 0);
        // Both have delivered 20 ms: two blocks go now, with no watermark.
        assert_eq!(m.mix(hns_of(4_800), -HNS_PER_SECOND, &mut out.writer()).unwrap(), 960);
        assert_eq!(out.writes, vec![480, 480]);
        assert!(out.samples.iter().all(|&s| s == level(0.375)), "{:?}", &out.samples[..4]);
        assert_eq!((m.stats.blocks, m.stats.released, m.stats.clipped), (2, 0, 0));
        // Left and right are summed separately.
        let mut m = Mixer::new(RATE, 2);
        let mut out = Out::default();
        let stereo: Vec<f32> = (0..BLOCK).flat_map(|_| [0.5, -0.5]).collect();
        m.push(0, &stereo, 0);
        m.push(1, &stereo, 0);
        m.mix(hns_of(BLOCK), 0, &mut out.writer()).unwrap();
        assert_eq!((out.samples[0], out.samples[1]), (level(1.0), level(-1.0)));
    }

    #[test]
    fn a_sum_past_full_scale_is_clamped_not_wrapped() {
        let mut m = Mixer::new(RATE, 3);
        let mut out = Out::default();
        for lane in 0..3 {
            m.push(lane, &constant(BLOCK, 0.6), 0);
        }
        m.mix(hns_of(BLOCK), 0, &mut out.writer()).unwrap();
        assert!(out.samples.iter().all(|&s| s == i16::MAX), "{:?}", &out.samples[..2]);
        assert_eq!(m.stats.clipped, BLOCK * 2);
        // And downwards.
        let mut m = Mixer::new(RATE, 2);
        let mut out = Out::default();
        m.push(0, &constant(BLOCK, -0.9), 0);
        m.push(1, &constant(BLOCK, -0.9), 0);
        m.mix(hns_of(BLOCK), 0, &mut out.writer()).unwrap();
        assert!(out.samples.iter().all(|&s| s == -i16::MAX));
    }

    /// A source that sends nothing (a quiet game, Discord's tree with nobody
    /// talking) holds the mix back only until the watermark passes, and then
    /// counts as silence.
    #[test]
    fn the_watermark_releases_a_silent_source() {
        let mut m = Mixer::new(RATE, 2);
        let mut out = Out::default();
        m.push(0, &constant(4_800, 0.5), 0);
        // The video is at 100 ms, the watermark not yet at 0: nothing, because
        // source 1 has delivered nothing.
        assert_eq!(m.mix(hns_of(4_800), hns_of(0), &mut out.writer()).unwrap(), 0);
        // The watermark at 45 ms releases four whole blocks, no more.
        assert_eq!(m.mix(hns_of(4_800), hns_of(2_160), &mut out.writer()).unwrap(), 1_920);
        assert_eq!(m.stats.released, 4);
        assert_eq!(m.missing(1), 1_920);
        assert!(out.samples.iter().all(|&s| s == level(0.5)));
        // It never mixes past the video, whatever the watermark says.
        assert_eq!(m.mix(hns_of(4_800), hns_of(48_000), &mut out.writer()).unwrap(), 2_880);
        assert_eq!(out.frames(), 4_800);
        assert_eq!(m.mix(hns_of(4_900), hns_of(48_000), &mut out.writer()).unwrap(), 0);
    }

    /// A source that joins late (the microphone's first packet, a Discord
    /// call that starts mid-game) is silence until it does, and in time from
    /// then. What it had for blocks already mixed is dropped.
    #[test]
    fn a_late_source_joins_in_place() {
        let mut m = Mixer::new(RATE, 2);
        let mut out = Out::default();
        m.push(0, &constant(9_600, 0.25), 0);
        m.mix(hns_of(9_600), hns_of(4_800), &mut out.writer()).unwrap();
        assert_eq!(out.frames(), 4_800);
        // Source 1's feed writes its lead silence and then its first packet
        // at 150 ms, contiguously from 0 as a feed does.
        m.push(1, &constant(7_200, 0.0), 0);
        m.push(1, &constant(2_400, 0.5), 7_200);
        assert_eq!(m.late(1), 4_800, "the part already mixed is late");
        m.mix(hns_of(9_600), hns_of(4_800), &mut out.writer()).unwrap();
        assert_eq!(out.frames(), 9_600);
        assert_eq!(out.at(4_799), level(0.25));
        assert_eq!(out.at(7_199), level(0.25));
        assert_eq!(out.at(7_200), level(0.75));
        assert_eq!(m.stats.released, 10, "only the first 100 ms waited on the watermark");
    }

    /// A microphone unplugged mid-game: its packets stop, the mix carries on
    /// with it silent, and nothing waits on it for longer than the watermark.
    #[test]
    fn an_unplugged_source_goes_silent_and_nothing_stalls() {
        let mut m = Mixer::new(RATE, 2);
        let mut out = Out::default();
        let mut now = 0u64;
        // Both deliver in 10 ms packets for half a second, then source 1
        // stops for good and source 0 carries on for another two seconds.
        for packet in 0..250u64 {
            m.push(0, &constant(BLOCK, 0.25), packet * BLOCK);
            if packet < 50 {
                m.push(1, &constant(BLOCK, 0.5), packet * BLOCK);
            }
            now += BLOCK;
            let watermark = hns_of(now) - LATENCY;
            m.mix(hns_of(now), watermark, &mut out.writer()).unwrap();
            // Never further behind the video than the watermark.
            assert!(out.frames() + 7_200 >= now, "stalled at {} of {now}", out.frames());
        }
        assert_eq!(out.at(24_000 - 1), level(0.75));
        assert_eq!(out.at(24_000), level(0.25));
        assert_eq!(m.finish(hns_of(now), &mut out.writer()).unwrap(), 7_200);
        assert_eq!(out.frames(), now);
        assert_eq!(out.at(now - 1), level(0.25));
        assert_eq!(m.missing(1), now - 24_000);
    }

    #[test]
    fn finish_lands_on_the_end_exactly_with_a_short_last_block() {
        let mut m = Mixer::new(RATE, 1);
        let mut out = Out::default();
        m.push(0, &constant(1_000, 0.5), 0);
        assert_eq!(m.finish(hns_of(1_100), &mut out.writer()).unwrap(), 1_100);
        assert_eq!(out.writes, vec![480, 480, 140]);
        assert_eq!(out.at(999), level(0.5));
        assert_eq!(out.at(1_000), 0);
        assert_eq!(m.missing(0), 100);
    }

    #[test]
    fn a_hole_or_a_repeat_from_a_lane_is_absorbed() {
        let mut m = Mixer::new(RATE, 1);
        let mut out = Out::default();
        m.push(0, &constant(240, 0.5), 0);
        // A hole of 240 frames, then a packet: the hole is silence.
        m.push(0, &constant(480, 0.25), 480);
        // A repeat of what it already has: dropped.
        m.push(0, &constant(480, 1.0), 480);
        assert_eq!(m.late(0), 0);
        m.mix(hns_of(960), 0, &mut out.writer()).unwrap();
        assert_eq!((out.at(239), out.at(240), out.at(480)), (level(0.5), 0, level(0.25)));
        // A lane given nothing at all, and a lane that does not exist.
        m.push(3, &constant(10, 1.0), 0);
        assert_eq!((m.late(3), m.missing(3)), (0, 0));
    }

    fn packet(frames_from_origin: u64, frames: u32, value: f32) -> Packet {
        Packet {
            hns: ORIGIN + hns_of(frames_from_origin),
            frames,
            discontinuity: false,
            pcm: vec![value; frames as usize * 2],
            clock: AudioClock::Qpc,
        }
    }

    /// The whole path the session drives, tick by tick on the 60 fps grid:
    /// two sources through their feeds into the mix, one of which joins late
    /// and is unplugged early, each packet handed over once it has been
    /// captured. The mix never runs past the video, never waits on the
    /// missing source for longer than the watermark, and ends on the last
    /// tick, with the packet in flight at stop cut to it.
    #[test]
    fn the_mixdown_mixes_aligned_sources_and_ends_with_the_video() {
        let mut mixdown = Mixdown::new(RATE, ORIGIN, 2);
        let mut out = Out::default();
        let mut next = 0u64;
        fn push(mixdown: &mut Mixdown, next: &mut u64) {
            mixdown.push(0, packet(*next * BLOCK, BLOCK as u32, 0.25));
            // Source 1 is heard from 0.5 s to 1.5 s only.
            if (50..150).contains(next) {
                mixdown.push(1, packet(*next * BLOCK, BLOCK as u32, 0.5));
            }
            *next += 1;
        }
        let ticks = 119u64;
        for k in 1..=ticks {
            let video_end = clock::tick_time(k, 60);
            let now = frames_at(video_end, RATE);
            while (next + 1) * BLOCK <= now {
                push(&mut mixdown, &mut next);
            }
            mixdown.write(video_end, video_end, &mut out.writer()).unwrap();
            assert!(out.frames() <= now, "past the video at tick {k}");
            assert!(out.frames() + 7_200 + BLOCK >= now, "stalled at tick {k}");
        }
        // Tick 119 ends at 95 200 frames, inside a block, and the packet
        // straddling it is still in flight when the stop arrives.
        let end = clock::tick_time(ticks, 60);
        push(&mut mixdown, &mut next);
        let pads = mixdown.finish(end, &mut out.writer()).unwrap();
        assert_eq!(out.frames(), 95_200);
        assert_eq!(pads[0], 0, "source 0's last packet was cut to the end, not padded");
        assert!(pads[1] > 0, "source 1 was padded");
        assert_eq!(out.at(24_000 - 1), level(0.25));
        assert_eq!(out.at(24_000), level(0.75));
        assert_eq!(out.at(72_000 - 1), level(0.75));
        assert_eq!(out.at(72_000), level(0.25));
        assert_eq!(out.at(95_199), level(0.25));
        // Each source's own aligner saw its own story: source 1 was held
        // while it was quiet, and joined across one gap, in place.
        let joined = &mixdown.feed(1).aligner().stats;
        assert!(joined.holds > 0 && joined.lead_silence > 0, "{joined:?}");
        assert_eq!((joined.gaps, joined.residual_worst), (1, 0), "{joined:?}");
        for source in 0..2 {
            assert_eq!(mixdown.feed(source).aligner().written(), 95_200, "source {source}");
            assert_eq!(mixdown.mixer().late(source), 0, "source {source}");
        }
        assert_eq!(mixdown.mixer().emitted(), 95_200);
    }

    /// The Game preset: one source, and a mix of one is that source.
    #[test]
    fn a_mix_of_one_source_is_the_source() {
        let mut mixdown = Mixdown::new(RATE, ORIGIN, 1);
        let mut out = Out::default();
        mixdown.push(0, packet(0, 480, 0.5));
        mixdown.push(0, packet(480, 480, -0.25));
        mixdown.write(hns_of(960), hns_of(960), &mut out.writer()).unwrap();
        assert_eq!(out.frames(), 960, "no watermark wait when every source is in");
        mixdown.finish(hns_of(1_000), &mut out.writer()).unwrap();
        assert_eq!((out.at(0), out.at(480), out.at(999)), (level(0.5), level(-0.25), 0));
        assert_eq!(mixdown.mixer().stats.released, 0);
    }
}
