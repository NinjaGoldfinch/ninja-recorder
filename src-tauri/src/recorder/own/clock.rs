//! The timing arithmetic, with no Windows in it.
//!
//! Ported whole from `spikes/p0c-video/src/clock.rs`, which measured it on the
//! box for #8 (DEVELOPMENT.md §16). Everything here is a pure function of
//! timestamps, so it is unit-tested on any host, and the numbers a Windows run
//! reports are not a property of the machine the arithmetic ran on.
//!
//! Two clocks meet in this file:
//!
//! - **Video** is placed on a constant-fps grid measured by the performance
//!   counter (QPC). Tick `k` is at `k / fps` seconds after the first captured
//!   frame, and its sample is whatever WGC last delivered. A tick with no new
//!   frame repeats the previous one, which is what holds the file at CFR.
//! - **Audio** is counted in samples by the device, whose crystal is not the
//!   one QPC runs on. WASAPI stamps every packet with the QPC time of its
//!   first frame, which is what lets the two be compared at all.
//!
//! [`Aligner`] measures the gap between those clocks (the *raw* drift, what a
//! pipeline that trusted the sample count would put in the file) and, in
//! [`AudioClock::Qpc`] mode, corrects it by slipping single frames so the
//! audio stays on the video's clock. [`AudioClock::Device`] is the fallback
//! for a source whose packets carry no usable QPC stamp: process loopback had
//! never been shown to, so the capture checks its first packet with
//! [`check_stamp`] before choosing. In that mode [`DeviceTimeline`] stamps the
//! packets from the sample count, anchored once at the first packet's
//! arrival, and the audio is then trusted. The endpoint the spike measured
//! drifted -0.2 ppm raw (DEVELOPMENT.md §16), which is what makes trusting it
//! tolerable.

/// Media Foundation's unit, 100 ns, and also the unit WASAPI's QPC positions
/// and WGC's `SystemRelativeTime` are reported in.
pub const HNS_PER_SECOND: i64 = 10_000_000;

/// The start of tick `k` on an exact `fps` grid, in 100 ns units.
///
/// Computed from `k` each time rather than accumulated, so ten minutes of
/// 166 666.67-unit intervals cannot round their way into a frame of error.
pub fn tick_time(k: u64, fps: u32) -> i64 {
    (i128::from(k) * i128::from(HNS_PER_SECOND) / i128::from(fps)) as i64
}

/// The index of the last tick whose start is at or before `rel`, or `None`
/// before tick 0.
pub fn ticks_due(rel: i64, fps: u32) -> Option<u64> {
    if rel < 0 {
        return None;
    }
    // The inverse of `tick_time`, rounded down, then nudged: integer division
    // in `tick_time` rounds down too, so the inverse can land one short.
    let mut k = (i128::from(rel) * i128::from(fps) / i128::from(HNS_PER_SECOND)) as u64;
    while tick_time(k + 1, fps) <= rel {
        k += 1;
    }
    Some(k)
}

/// `hns` as a count of video frames at `fps`.
pub fn hns_to_frames(hns: i64, fps: u32) -> f64 {
    hns as f64 * f64::from(fps) / HNS_PER_SECOND as f64
}

/// `samples` at `rate` as a count of video frames at `fps`.
pub fn samples_to_frames(samples: i64, rate: u32, fps: u32) -> f64 {
    samples as f64 * f64::from(fps) / f64::from(rate)
}

/// `samples` at `rate` in milliseconds.
pub fn samples_to_ms(samples: i64, rate: u32) -> f64 {
    samples as f64 * 1000.0 / f64::from(rate)
}

/// Where a packet's first frame belongs, in samples from the video's origin,
/// rounded to the nearest sample. Rounding rather than truncating matters:
/// a 100 ns stamp is a fifth of a sample at 48 kHz, and truncation toward
/// zero would bias every negative position by one.
pub fn samples_at(rel_hns: i64, rate: u32) -> i64 {
    let hns = i128::from(HNS_PER_SECOND);
    ((i128::from(rel_hns) * i128::from(rate) + hns / 2).div_euclid(hns)) as i64
}

/// Whose clock the audio's timestamps follow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioClock {
    /// Placed on QPC, the video's clock, by slipping single frames. What the
    /// shipping pipeline has to do in some form, and the default.
    Qpc,
    /// The device's sample count, uncorrected. Aligned once at the start and
    /// then trusted, so the file's end offset is the raw drift itself.
    ///
    /// The fallback for a source whose packets carry no QPC stamp (#237):
    /// no frame is slipped, so a caller that stamps packets from the sample
    /// count ([`DeviceTimeline`]) gets them appended back to back. Only a
    /// hole or an overlap wider than 50 ms moves the audio, because that is a
    /// stream that stopped delivering, not drift. The raw-drift figures are
    /// then meaningless and should not be reported.
    Device,
}

impl AudioClock {
    pub fn name(self) -> &'static str {
        match self {
            AudioClock::Qpc => "qpc",
            AudioClock::Device => "device",
        }
    }
}

/// What to do with one packet before appending it.
///
/// Applied in order: write `silence` zero frames, then the packet's frame at
/// index `skip` repeated `repeat` times, then the packet from `skip` onwards.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Placement {
    pub silence: u64,
    pub repeat: u32,
    pub skip: u32,
}

impl Placement {
    /// How many frames this placement appends for a packet of `frames`.
    pub fn appended(&self, frames: u32) -> u64 {
        self.silence + u64::from(self.repeat) + u64::from(frames.saturating_sub(self.skip))
    }
}

/// The counters the report prints. All sample counts are at the device rate.
#[derive(Clone, Debug, Default)]
pub struct AlignStats {
    pub packets: u64,
    pub received: u64,
    /// Packets WASAPI flagged `AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY`: a
    /// glitch in the engine, and a jump in the sample count that is not
    /// drift. The raw drift is re-anchored across each one.
    pub discontinuities: u64,
    /// The device clock minus QPC since the first packet, in samples:
    /// positive means the device delivered more samples than the elapsed
    /// time accounts for. This is the drift a pipeline would carry if it
    /// timestamped by sample count.
    pub raw_drift_last: i64,
    pub raw_drift_worst: i64,
    /// QPC time of the first packet counted, relative to the video origin.
    pub first_rel: Option<i64>,
    /// QPC time of the last packet counted, relative to the video origin.
    pub last_rel: i64,
    /// Audio already written minus where the packet belongs, sampled after
    /// the placement: the A/V misalignment the file actually carries, at
    /// packet granularity.
    pub residual_last: i64,
    pub residual_worst: i64,
    pub slips_dropped: u64,
    pub slips_repeated: u64,
    /// Holes wider than the gap threshold, filled with silence.
    pub gaps: u64,
    pub gap_samples: u64,
    /// Overlaps wider than the gap threshold, dropped.
    pub overlaps: u64,
    pub overlap_samples: u64,
    /// Samples discarded because they were captured before the video began.
    pub lead_dropped: u64,
    /// Silence written at the start because audio began after the video.
    pub lead_silence: u64,
    /// Times [`Aligner::hold`] wrote silence because no packet had come, and
    /// how much. A process-loopback stream is not guaranteed to be fed while
    /// its target is silent.
    pub holds: u64,
    pub held_samples: u64,
}

/// Places audio packets on the video's timeline and measures the clocks.
pub struct Aligner {
    rate: u32,
    clock: AudioClock,
    /// Samples appended to the file so far.
    written: u64,
    /// Offset subtracted from the raw drift so it reads zero at the first
    /// packet and carries across discontinuities without jumping.
    raw_baseline: i64,
    /// Samples received since the first counted packet.
    received_since_first: i64,
    first_position: Option<i64>,
    pub stats: AlignStats,
}

impl Aligner {
    pub fn new(rate: u32, clock: AudioClock) -> Self {
        Aligner {
            rate,
            clock,
            written: 0,
            raw_baseline: 0,
            received_since_first: 0,
            first_position: None,
            stats: AlignStats::default(),
        }
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }

    pub fn written(&self) -> u64 {
        self.written
    }

    /// Half a millisecond. A misalignment below this is left alone; above it
    /// one frame is slipped per packet, which at 10 ms packets corrects up to
    /// about 2000 ppm, some fifty times what a sound card is expected to be off
    /// by.
    fn slip_threshold(&self) -> i64 {
        i64::from(self.rate / 2000).max(1)
    }

    /// 50 ms. Past this the stream has a hole or an overlap, not drift, and
    /// slipping one frame per packet would take seconds to close it.
    fn gap_threshold(&self) -> i64 {
        i64::from(self.rate / 20).max(1)
    }

    /// Decide where a packet of `frames` goes. `rel_hns` is the QPC time of
    /// its first frame relative to the video origin (tick 0), which can be
    /// negative for audio captured before the first video frame.
    pub fn place(&mut self, frames: u32, rel_hns: i64, discontinuity: bool) -> Placement {
        let position = samples_at(rel_hns, self.rate);
        let end = position + i64::from(frames);

        // Before the origin entirely: nothing of it belongs in the file.
        if end <= 0 {
            self.stats.lead_dropped += u64::from(frames);
            return Placement {
                skip: frames,
                ..Placement::default()
            };
        }

        self.measure_raw(frames, position, rel_hns, discontinuity);

        let mut placement = Placement::default();
        if self.written == 0 && self.stats.lead_silence == 0 {
            // The first packet that reaches the file lines up with the origin
            // exactly, in either mode: drop what came before tick 0, or pad
            // up to where this packet begins.
            if position < 0 {
                placement.skip = (-position) as u32;
                self.stats.lead_dropped += u64::from(placement.skip);
            } else {
                placement.silence = position as u64;
                self.stats.lead_silence = placement.silence;
            }
        } else {
            // Holes and overlaps are handled in either mode: past the gap
            // threshold the stream stopped delivering or restarted, which is
            // not drift. Slips below it are the QPC mode's alone.
            let error = self.written as i64 - position;
            let qpc = self.clock == AudioClock::Qpc;
            if error < -self.gap_threshold() {
                placement.silence = (-error) as u64;
                self.stats.gaps += 1;
                self.stats.gap_samples += placement.silence;
            } else if error > self.gap_threshold() {
                placement.skip = error.min(i64::from(frames)) as u32;
                self.stats.overlaps += 1;
                self.stats.overlap_samples += u64::from(placement.skip);
            } else if qpc && error > self.slip_threshold() && frames > 1 {
                placement.skip = 1;
                self.stats.slips_dropped += 1;
            } else if qpc && error < -self.slip_threshold() && frames > 0 {
                placement.repeat = 1;
                self.stats.slips_repeated += 1;
            }
        }

        // The residual is measured where this packet's first kept frame lands
        // against where it belongs, which is the misalignment a viewer would
        // see at that moment.
        let landed = self.written as i64 + placement.silence as i64 + i64::from(placement.repeat);
        let belongs = position + i64::from(placement.skip);
        let residual = landed - belongs;
        self.stats.residual_last = residual;
        if residual.abs() > self.stats.residual_worst.abs() {
            self.stats.residual_worst = residual;
        }

        self.written += placement.appended(frames);
        placement
    }

    fn measure_raw(&mut self, frames: u32, position: i64, rel_hns: i64, discontinuity: bool) {
        self.stats.packets += 1;
        self.stats.received += u64::from(frames);
        self.stats.last_rel = rel_hns;
        if discontinuity {
            self.stats.discontinuities += 1;
        }

        let Some(first) = self.first_position else {
            self.first_position = Some(position);
            self.stats.first_rel = Some(rel_hns);
            self.received_since_first = i64::from(frames);
            return;
        };

        let offset = self.received_since_first - (position - first);
        if discontinuity {
            // A glitch lost or repeated samples; that is a jump, not drift.
            // Move the baseline so the raw figure carries on from where it was.
            self.raw_baseline = offset - self.stats.raw_drift_last;
        }
        let drift = offset - self.raw_baseline;
        self.stats.raw_drift_last = drift;
        if drift.abs() > self.stats.raw_drift_worst.abs() {
            self.stats.raw_drift_worst = drift;
        }
        self.received_since_first += i64::from(frames);
    }

    /// How much silence to append so the audio ends where the video does,
    /// at `end_rel_hns`. Returns `(pad, overhang)`: the frames to append, and
    /// how far the audio already runs past the end (which is left alone,
    /// because an encoder cannot take samples back).
    pub fn finish(&mut self, end_rel_hns: i64) -> (u64, u64) {
        let target = samples_at(end_rel_hns, self.rate).max(0) as u64;
        if self.written < target {
            let pad = target - self.written;
            self.written = target;
            (pad, 0)
        } else {
            (0, self.written - target)
        }
    }

    /// Writes silence up to `until_rel_hns` if the audio has fallen behind
    /// it, and returns how many frames to append. The caller holds audio this
    /// way when no packet has come for a while, with `until` a margin behind
    /// the video, so a quiet source does not leave the muxer waiting on one
    /// track while the other runs ahead. A packet that turns up later for
    /// time already filled is an overlap, and [`Aligner::place`] drops that
    /// part of it.
    pub fn hold(&mut self, until_rel_hns: i64) -> u64 {
        let target = samples_at(until_rel_hns, self.rate).max(0) as u64;
        if self.written >= target {
            return 0;
        }
        let silence = target - self.written;
        if self.written == 0 {
            // Audio that has not begun by now began after the video: the
            // same lead silence the first packet would have written.
            self.stats.lead_silence = silence;
        }
        self.written = target;
        self.stats.holds += 1;
        self.stats.held_samples += silence;
        silence
    }

    pub fn clock(&self) -> AudioClock {
        self.clock
    }

    /// Changes whose clock placement follows. The source decides on its
    /// first packet ([`check_stamp`]), which can be after the aligner was
    /// made.
    pub fn set_clock(&mut self, clock: AudioClock) {
        self.clock = clock;
    }

    /// The raw drift as a rate, in parts per million, over the span the
    /// packets covered. `None` until there is a span to divide by.
    pub fn raw_ppm(&self) -> Option<f64> {
        let first = self.stats.first_rel?;
        let span = self.stats.last_rel - first;
        if span <= 0 {
            return None;
        }
        let span_samples = span as f64 * f64::from(self.rate) / HNS_PER_SECOND as f64;
        Some(self.stats.raw_drift_last as f64 / span_samples * 1e6)
    }
}

/// What a packet's QPC stamp turned out to be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stamp {
    /// A QPC time plausibly the packet's first frame: at or before the
    /// moment the packet was taken, and not long before. `lag` is how long
    /// before, in 100 ns units.
    Qpc { lag: i64 },
    /// No stamp at all. What the plan feared process loopback would give.
    Zero,
    /// Non-zero, but not the performance counter this process reads: in the
    /// future, or older than [`STAMP_MAX_LAG`].
    Implausible { lag: i64 },
}

/// How old a stamp may be when its packet is taken and still count as QPC.
/// A shared-mode packet is about 10 ms, and the engine hands it over within
/// a period or two; a second allows for a thread that was descheduled, and
/// is far closer than any other clock's zero would land.
pub const STAMP_MAX_LAG: i64 = HNS_PER_SECOND;

/// A stamp a little after the moment it was read is rounding between two
/// conversions of the same counter, not the future.
const STAMP_TOLERANCE: i64 = 10_000; // 1 ms

/// Whether `stamp_hns`, from `GetBuffer`'s `pu64QPCPosition`, is on the
/// performance counter, judged against `arrival_hns`, the counter read just
/// after `GetBuffer` returned.
pub fn check_stamp(stamp_hns: u64, arrival_hns: i64) -> Stamp {
    if stamp_hns == 0 {
        return Stamp::Zero;
    }
    let lag = arrival_hns.saturating_sub(i64::try_from(stamp_hns).unwrap_or(i64::MAX));
    if (-STAMP_TOLERANCE..=STAMP_MAX_LAG).contains(&lag) {
        Stamp::Qpc { lag }
    } else {
        Stamp::Implausible { lag }
    }
}

/// Past this, a packet arriving later than the sample count says it should
/// is a stream that stopped and restarted, not jitter. Twice the aligner's
/// gap threshold, because arrival times jitter by a period or two where a
/// real stamp does not.
pub const DEVICE_HOLE: i64 = HNS_PER_SECOND / 10; // 100 ms

/// Stamps packets from the device's sample count, for a source whose own
/// stamps are not QPC ([`AudioClock::Device`]).
///
/// The first packet is anchored where its arrival says it began: the QPC
/// read when it was taken, less its own length. Every later packet is placed
/// that anchor plus the frames counted since, so the device's clock is
/// trusted and nothing is slipped. The one exception is a packet that
/// arrives more than [`DEVICE_HOLE`] later than that: the stream stopped
/// delivering (process loopback may go quiet while its target is silent),
/// so the timeline re-anchors at the packet and reports a discontinuity,
/// which the aligner fills with silence.
pub struct DeviceTimeline {
    rate: u32,
    anchor: Option<i64>,
    counted: i64,
    /// How many times the timeline re-anchored across a hole.
    pub reanchors: u64,
}

impl DeviceTimeline {
    pub fn new(rate: u32) -> Self {
        DeviceTimeline { rate, anchor: None, counted: 0, reanchors: 0 }
    }

    fn span(&self, frames: i64) -> i64 {
        (i128::from(frames) * i128::from(HNS_PER_SECOND) / i128::from(self.rate)) as i64
    }

    /// The stamp for a packet of `frames` taken at `arrival_hns`, and
    /// whether it follows a hole.
    pub fn stamp(&mut self, frames: u32, arrival_hns: i64) -> (i64, bool) {
        let began = arrival_hns - self.span(i64::from(frames));
        let Some(anchor) = self.anchor else {
            self.anchor = Some(began);
            self.counted = i64::from(frames);
            return (began, false);
        };
        let expected = anchor + self.span(self.counted);
        if began - expected > DEVICE_HOLE {
            self.anchor = Some(began);
            self.counted = i64::from(frames);
            self.reanchors += 1;
            return (began, true);
        }
        self.counted += i64::from(frames);
        (expected, false)
    }
}

/// Chooses a source's clock from its first packet, and stamps every packet
/// on it: what a capture thread runs each packet through before sending it.
///
/// **The question is answered at runtime, per source, per recording.** If
/// the first packet's `pu64QPCPosition` passes [`check_stamp`], the source
/// is on QPC and the aligner holds it to the video by slipping frames. If it
/// is 0, or not this process's counter, the source goes to
/// [`AudioClock::Device`] and [`DeviceTimeline`] stamps it from the sample
/// count. The choice is made once: a stream whose clock changed halfway
/// would move its audio by however far apart the two had drifted.
///
/// In QPC mode a later packet whose stamp fails the check is stamped from
/// its arrival instead, and counted in [`Stamper::substituted`]: one bad
/// stamp should cost that packet's placement, not the recording's clock.
pub struct Stamper {
    rate: u32,
    clock: Option<AudioClock>,
    timeline: DeviceTimeline,
    /// What the first packet's stamp was: the answer to the question.
    pub first: Option<Stamp>,
    /// QPC-mode packets whose own stamp failed the check.
    pub substituted: u64,
}

impl Stamper {
    pub fn new(rate: u32) -> Self {
        Stamper {
            rate,
            clock: None,
            timeline: DeviceTimeline::new(rate),
            first: None,
            substituted: 0,
        }
    }

    /// The clock chosen, once the first packet has been seen.
    pub fn clock(&self) -> Option<AudioClock> {
        self.clock
    }

    pub fn reanchors(&self) -> u64 {
        self.timeline.reanchors
    }

    fn began_at(&self, frames: u32, arrival_hns: i64) -> i64 {
        let span = i128::from(frames) * i128::from(HNS_PER_SECOND) / i128::from(self.rate);
        arrival_hns - span as i64
    }

    /// The stamp for a packet of `frames` whose `GetBuffer` gave `qpc_stamp`,
    /// taken at `arrival_hns`; whether it follows a hole in device time; and
    /// the clock it is on.
    ///
    /// `qpc_stamp` is `None` for a packet the engine flagged
    /// `AUDCLNT_BUFFERFLAGS_TIMESTAMP_ERROR`, which WASAPI documents for the
    /// first packet after a start among others. Such a packet is stamped from
    /// its arrival (or the count, in device mode), counted as substituted,
    /// and never decides the clock: the first unflagged packet does.
    pub fn stamp(
        &mut self,
        frames: u32,
        qpc_stamp: Option<u64>,
        arrival_hns: i64,
    ) -> (i64, bool, AudioClock) {
        let Some(qpc_stamp) = qpc_stamp else {
            self.substituted += 1;
            return match self.clock {
                Some(AudioClock::Device) => {
                    let (hns, hole) = self.timeline.stamp(frames, arrival_hns);
                    (hns, hole, AudioClock::Device)
                }
                // Undecided, the aligner places a first packet the same way
                // in either mode, so QPC is as good a label as any.
                Some(AudioClock::Qpc) | None => {
                    (self.began_at(frames, arrival_hns), false, AudioClock::Qpc)
                }
            };
        };
        let check = check_stamp(qpc_stamp, arrival_hns);
        let clock = match self.clock {
            Some(clock) => clock,
            None => {
                let clock = match check {
                    Stamp::Qpc { .. } => AudioClock::Qpc,
                    Stamp::Zero | Stamp::Implausible { .. } => AudioClock::Device,
                };
                self.first = Some(check);
                self.clock = Some(clock);
                clock
            }
        };
        match clock {
            AudioClock::Qpc => match check {
                Stamp::Qpc { .. } => (qpc_stamp as i64, false, clock),
                Stamp::Zero | Stamp::Implausible { .. } => {
                    self.substituted += 1;
                    (self.began_at(frames, arrival_hns), false, clock)
                }
            },
            AudioClock::Device => {
                let (hns, hole) = self.timeline.stamp(frames, arrival_hns);
                (hns, hole, clock)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;
    const PACKET: u32 = 480; // 10 ms

    fn hns_of(samples: i64) -> i64 {
        samples * HNS_PER_SECOND / i64::from(RATE)
    }

    #[test]
    fn ticks_land_on_an_exact_grid() {
        assert_eq!(tick_time(0, 60), 0);
        assert_eq!(tick_time(60, 60), HNS_PER_SECOND);
        // Ten minutes: no accumulated rounding.
        assert_eq!(tick_time(36_000, 60), 600 * HNS_PER_SECOND);
        assert_eq!(tick_time(1, 60), 166_666);
        assert_eq!(tick_time(2, 60), 333_333);
    }

    #[test]
    fn ticks_due_inverts_tick_time() {
        assert_eq!(ticks_due(-1, 60), None);
        assert_eq!(ticks_due(0, 60), Some(0));
        for k in [1u64, 2, 3, 59, 60, 61, 35_999, 36_000] {
            let t = tick_time(k, 60);
            assert_eq!(ticks_due(t, 60), Some(k), "at tick {k}");
            assert_eq!(ticks_due(t - 1, 60), Some(k - 1), "just before tick {k}");
        }
    }

    #[test]
    fn a_perfect_clock_has_no_drift_and_no_corrections() {
        let mut a = Aligner::new(RATE, AudioClock::Qpc);
        for i in 0..60_000i64 {
            let p = a.place(PACKET, hns_of(i * i64::from(PACKET)), false);
            assert_eq!(p, Placement::default(), "packet {i}");
        }
        assert_eq!(a.stats.raw_drift_worst, 0);
        assert_eq!(a.stats.residual_worst, 0);
        assert_eq!(a.stats.slips_dropped + a.stats.slips_repeated, 0);
        assert_eq!(a.written(), 60_000 * u64::from(PACKET));
    }

    /// A device running 50 ppm fast delivers its samples early by QPC: after
    /// ten minutes it is 30 ms ahead, which is almost two frames at 60 fps.
    fn fast_device(clock: AudioClock) -> Aligner {
        let mut a = Aligner::new(RATE, clock);
        let packets = 60_000i64; // ten minutes of 10 ms packets
        for i in 0..packets {
            let device_samples = i * i64::from(PACKET);
            // QPC time at which the device had produced that many samples.
            let rel = (device_samples as f64 / (1.0 + 50e-6) * HNS_PER_SECOND as f64
                / f64::from(RATE)) as i64;
            a.place(PACKET, rel, false);
        }
        a
    }

    #[test]
    fn raw_drift_is_measured_whichever_clock_is_written() {
        for clock in [AudioClock::Qpc, AudioClock::Device] {
            let a = fast_device(clock);
            let ms = samples_to_ms(a.stats.raw_drift_last, RATE);
            assert!((ms - 30.0).abs() < 0.5, "{clock:?}: {ms} ms");
            let ppm = a.raw_ppm().unwrap();
            assert!((ppm - 50.0).abs() < 1.0, "{clock:?}: {ppm} ppm");
        }
    }

    #[test]
    fn the_device_clock_carries_the_drift_into_the_file() {
        let a = fast_device(AudioClock::Device);
        let frames = samples_to_frames(a.stats.residual_last, RATE, 60);
        assert!(frames > 1.5, "residual {frames} frames");
        assert_eq!(a.stats.slips_dropped, 0);
    }

    #[test]
    fn the_qpc_clock_holds_the_residual_under_the_slip_threshold() {
        let a = fast_device(AudioClock::Qpc);
        // One frame more than the threshold, at most: a packet is corrected
        // after it has crossed it.
        assert!(
            a.stats.residual_worst.abs() <= 25,
            "{}",
            a.stats.residual_worst
        );
        assert!(a.stats.slips_dropped > 1_000, "{}", a.stats.slips_dropped);
        assert_eq!(a.stats.slips_repeated, 0);
        assert_eq!(a.stats.gaps + a.stats.overlaps, 0);
    }

    #[test]
    fn a_slow_device_is_corrected_by_repeating() {
        let mut a = Aligner::new(RATE, AudioClock::Qpc);
        for i in 0..60_000i64 {
            let device_samples = i * i64::from(PACKET);
            let rel = (device_samples as f64 / (1.0 - 50e-6) * HNS_PER_SECOND as f64
                / f64::from(RATE)) as i64;
            a.place(PACKET, rel, false);
        }
        assert!(a.stats.slips_repeated > 1_000);
        assert!(a.stats.residual_worst.abs() <= 25);
        assert!(samples_to_ms(a.stats.raw_drift_last, RATE) < -29.0);
    }

    #[test]
    fn audio_before_the_origin_is_dropped_and_after_it_is_padded() {
        let mut early = Aligner::new(RATE, AudioClock::Qpc);
        // Entirely before tick 0.
        assert_eq!(early.place(PACKET, hns_of(-1000), false).skip, PACKET);
        // Straddling it: the 100 samples before 0 go.
        let p = early.place(PACKET, hns_of(-100), false);
        assert_eq!(p.skip, 100);
        assert_eq!(early.written(), 380);
        assert_eq!(early.stats.residual_last, 0);

        let mut late = Aligner::new(RATE, AudioClock::Device);
        let p = late.place(PACKET, hns_of(2_400), false);
        assert_eq!(p.silence, 2_400);
        assert_eq!(late.written(), 2_880);
        assert_eq!(late.stats.residual_last, 0);
    }

    #[test]
    fn a_hole_is_filled_with_silence_and_not_counted_as_drift() {
        let mut a = Aligner::new(RATE, AudioClock::Qpc);
        a.place(PACKET, 0, false);
        // 200 ms go missing, and the engine says so.
        let p = a.place(PACKET, hns_of(480 + 9_600), true);
        assert_eq!(p.silence, 9_600);
        assert_eq!(a.stats.gaps, 1);
        assert_eq!(a.stats.discontinuities, 1);
        assert_eq!(a.stats.raw_drift_last, 0);
        let p = a.place(PACKET, hns_of(480 + 9_600 + 480), false);
        assert_eq!(p, Placement::default());
    }

    #[test]
    fn unit_conversions_and_names() {
        assert_eq!(hns_to_frames(HNS_PER_SECOND, 60), 60.0);
        assert_eq!(samples_to_frames(i64::from(RATE), RATE, 60), 60.0);
        assert_eq!(samples_to_ms(480, RATE), 10.0);
        assert_eq!(AudioClock::Qpc.name(), "qpc");
        assert_eq!(AudioClock::Device.name(), "device");
        assert_eq!(Aligner::new(RATE, AudioClock::Qpc).rate(), RATE);
    }

    #[test]
    fn finish_pads_to_the_video_end() {
        let mut a = Aligner::new(RATE, AudioClock::Qpc);
        a.place(PACKET, 0, false);
        assert_eq!(a.finish(hns_of(1_000)), (520, 0));
        assert_eq!(a.written(), 1_000);
        assert_eq!(a.finish(hns_of(900)), (0, 100));
    }

    #[test]
    fn the_device_clock_fills_a_hole_but_never_slips() {
        let mut a = Aligner::new(RATE, AudioClock::Device);
        a.place(PACKET, 0, false);
        // 30 ms late: under the gap threshold, so trusted as it is.
        assert_eq!(a.place(PACKET, hns_of(480 + 1_440), false), Placement::default());
        // 200 ms late: a stream that stopped, filled with silence.
        let p = a.place(PACKET, hns_of(960 + 9_600), true);
        assert_eq!(p.silence, 9_600);
        assert_eq!((a.stats.gaps, a.stats.slips_dropped + a.stats.slips_repeated), (1, 0));
    }

    #[test]
    fn hold_fills_a_quiet_source_and_a_late_packet_overlaps_it() {
        let mut a = Aligner::new(RATE, AudioClock::Qpc);
        // Nothing yet at 100 ms: hold to 100 ms, counted as the lead.
        assert_eq!(a.hold(hns_of(4_800)), 4_800);
        assert_eq!(a.stats.lead_silence, 4_800);
        assert_eq!(a.hold(hns_of(4_000)), 0, "never backwards");
        // A packet from 90 ms: within the gap threshold of what was held,
        // so it is slipped rather than cut.
        let p = a.place(PACKET, hns_of(4_320), false);
        assert_eq!((p.skip, p.silence), (1, 0));
        // Hold again past a long quiet stretch, then a packet from inside it:
        // the part already filled is dropped as an overlap.
        a.hold(hns_of(48_000));
        let p = a.place(PACKET, hns_of(48_000 - 4_800), false);
        assert_eq!(p.skip, PACKET);
        assert_eq!(a.stats.overlaps, 1);
        assert_eq!((a.stats.holds, a.stats.held_samples), (2, 4_800 + 48_000 - 5_279));
    }

    #[test]
    fn stamps_are_checked_against_the_moment_the_packet_was_taken() {
        let now = 1_000 * HNS_PER_SECOND;
        assert_eq!(check_stamp(0, now), Stamp::Zero);
        assert_eq!(check_stamp((now - 100_000) as u64, now), Stamp::Qpc { lag: 100_000 });
        // Rounding between two conversions of the counter is not the future.
        assert_eq!(check_stamp((now + 5_000) as u64, now), Stamp::Qpc { lag: -5_000 });
        assert_eq!(
            check_stamp((now + HNS_PER_SECOND) as u64, now),
            Stamp::Implausible { lag: -HNS_PER_SECOND }
        );
        assert_eq!(
            check_stamp((now - 5 * HNS_PER_SECOND) as u64, now),
            Stamp::Implausible { lag: 5 * HNS_PER_SECOND }
        );
        // A device position in frames is not a QPC time.
        assert!(matches!(check_stamp(96_000, now), Stamp::Implausible { .. }));
        assert!(matches!(check_stamp(u64::MAX, now), Stamp::Implausible { .. }));
    }

    #[test]
    fn the_device_timeline_counts_from_the_first_arrival_and_reanchors_across_a_hole() {
        let mut t = DeviceTimeline::new(RATE);
        let t0 = 50 * HNS_PER_SECOND;
        // The first packet began one packet before it was taken.
        assert_eq!(t.stamp(PACKET, t0), (t0 - hns_of(480), false));
        // Arrival jitter does not move the count.
        assert_eq!(t.stamp(PACKET, t0 + hns_of(480) + 300_000), (t0, false));
        assert_eq!(t.stamp(PACKET, t0 + hns_of(960)), (t0 + hns_of(480), false));
        // A second of nothing, then packets again: re-anchored.
        let back = t0 + hns_of(960) + HNS_PER_SECOND + hns_of(480);
        assert_eq!(t.stamp(PACKET, back), (back - hns_of(480), true));
        assert_eq!(t.reanchors, 1);
        assert_eq!(t.stamp(PACKET, back + hns_of(480)), (back, false));
    }

    #[test]
    fn a_device_timeline_through_the_aligner_is_seamless_and_fills_the_hole() {
        let mut t = DeviceTimeline::new(RATE);
        let mut a = Aligner::new(RATE, AudioClock::Device);
        let origin = 10 * HNS_PER_SECOND;
        for i in 0..100i64 {
            // Arrivals jitter by up to 4 ms, one way only.
            let arrival = origin + hns_of(480 * (i + 1)) + (i % 3) * 20_000;
            let (stamp, hole) = t.stamp(PACKET, arrival);
            assert_eq!(a.place(PACKET, stamp - origin, hole), Placement::default(), "{i}");
        }
        assert_eq!(a.written(), 100 * 480);
        // Half a second with no packets, then the stream resumes.
        let arrival = origin + hns_of(480 * 101) + HNS_PER_SECOND / 2;
        let (stamp, hole) = t.stamp(PACKET, arrival);
        let p = a.place(PACKET, stamp - origin, hole);
        assert!(hole && p.silence > 20_000, "{p:?}");
        assert_eq!(a.stats.residual_last, 0);
    }

    #[test]
    fn real_stamps_put_the_source_on_qpc_and_a_bad_one_is_substituted() {
        let mut s = Stamper::new(RATE);
        assert_eq!(s.clock(), None);
        let now = 500 * HNS_PER_SECOND;
        let stamp = (now - 150_000) as u64;
        assert_eq!(s.stamp(PACKET, Some(stamp), now), (stamp as i64, false, AudioClock::Qpc));
        assert_eq!(s.first, Some(Stamp::Qpc { lag: 150_000 }));
        // One zero stamp later on costs that packet only.
        let later = now + hns_of(480);
        let from_arrival = (later - hns_of(480), false, AudioClock::Qpc);
        assert_eq!(s.stamp(PACKET, Some(0), later), from_arrival);
        assert_eq!(s.substituted, 1);
        assert_eq!(s.clock(), Some(AudioClock::Qpc));
    }

    #[test]
    fn zero_stamps_put_the_source_on_device_time_for_good() {
        let mut s = Stamper::new(RATE);
        let now = 500 * HNS_PER_SECOND;
        let first = (now - hns_of(480), false, AudioClock::Device);
        assert_eq!(s.stamp(PACKET, Some(0), now), first);
        assert_eq!(s.first, Some(Stamp::Zero));
        // A real-looking stamp later does not switch the clock back.
        let later = now + hns_of(480);
        let (hns, _, clock) = s.stamp(PACKET, Some(later as u64), later);
        assert_eq!((hns, clock), (now, AudioClock::Device));
        // A flagged one in device mode is still counted into the timeline.
        let (hns, _, clock) = s.stamp(PACKET, None, later + hns_of(480));
        assert_eq!((hns, clock), (later, AudioClock::Device));
        assert_eq!((s.substituted, s.reanchors()), (1, 0));
        // And a stamp on some other clock is treated as none.
        let mut s = Stamper::new(RATE);
        s.stamp(PACKET, Some(12_345), now);
        assert!(matches!(s.first, Some(Stamp::Implausible { .. })));
        assert_eq!(s.clock(), Some(AudioClock::Device));
    }

    #[test]
    fn a_flagged_first_packet_does_not_decide_the_clock() {
        let mut s = Stamper::new(RATE);
        let now = 500 * HNS_PER_SECOND;
        // WASAPI may flag the first packet after a start as a timestamp
        // error: it is placed from its arrival and the question stays open.
        assert_eq!(s.stamp(PACKET, None, now), (now - hns_of(480), false, AudioClock::Qpc));
        assert_eq!((s.clock(), s.first, s.substituted), (None, None, 1));
        let later = now + hns_of(480);
        let stamp = (later - hns_of(480) - 20_000) as u64;
        assert_eq!(s.stamp(PACKET, Some(stamp), later).2, AudioClock::Qpc);
        assert_eq!(s.first, Some(Stamp::Qpc { lag: hns_of(480) + 20_000 }));
    }
}
