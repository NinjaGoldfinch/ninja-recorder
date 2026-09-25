//! A fragmented-MP4 writer for one H.264 video track and any number of AAC
//! tracks (WS1.6.3, #235).
//!
//! Media Foundation's MP4 sinks take one video and **one** audio stream, and
//! every audio preset except Game writes 2-4 tracks, so the own backend
//! writes its files itself (DEVELOPMENT.md §2.5). Nothing here calls Windows:
//! it is built and tested on Linux, with ffprobe and a full ffmpeg decode
//! checking every file it writes.
//!
//! # The file
//!
//! ```text
//! ftyp
//! moov   mvhd, trak per track (tkhd, mdia: mdhd, hdlr, minf: vmhd|smhd, dinf,
//!        stbl: stsd[avc1+avcC | mp4a+esds] and empty stts/stsc/stsz/stco),
//!        mvex: mehd, trex per track
//! moof   mfhd, traf per track with samples (tfhd, tfdt v1, trun)
//! mdat   that fragment's samples, track by track in traf order
//! ...    moof + mdat, once per flush_fragment()
//! mfra   tfra per track, mfro          (finish() or repair())
//! ```
//!
//! # Timescales
//!
//! Every timestamp the caller passes is in its **track's** timescale:
//!
//! - **Video: 90 kHz** ([`VIDEO_TIMESCALE`]). Every common frame period is a
//!   whole number of ticks in it (60 fps is 1500, 30 is 3000, 144 is 625,
//!   29.97 is 3003), and so is a millisecond.
//! - **Audio: the sample rate**, so an AAC frame is exactly 1024 ticks and
//!   never accumulates rounding.
//! - The movie timescale, used only by `mehd`, is 1 kHz ([`MOVIE_TIMESCALE`]).
//!
//! # Timestamps, and B-frames
//!
//! Samples arrive in **decode order**. A track's decode time starts at its
//! first sample's `pts` and then advances by each sample's `duration`, so
//! `duration` must be the gap to the next sample's decode time (for a stream
//! without B-frames that is simply `next pts - pts`). The composition offset
//! is `pts - dts`, written per sample in `trun`.
//!
//! **B-frames are supported.** With reordering, some offsets come out
//! negative, and those fragments use `trun` version 1 (signed offsets), the
//! CMAF convention in which the first frame's offset is zero and no edit list
//! is needed. Media Foundation's H.264 encoder in low-latency mode emits no
//! B-frames, so production never takes that path; the tests exercise it with
//! x264's `-bf 2` anyway, because a hardware MFT outside low-latency mode is
//! allowed to emit them.
//!
//! One caveat, measured with ffmpeg 7.0.2: its MP4 demuxer handles negative
//! offsets by moving every *pts* of that track later by the most negative
//! offset (`dts_shift`), rather than moving dts earlier. With x264's
//! `-bf 2` that is one frame, so an ffmpeg-based player (which includes
//! WebView2 playing a file directly) starts the video 16.7 ms after the audio.
//! Decoding is unaffected. Fixing it needs an `elst` giving the reorder delay,
//! which `moov` must declare before the first sample, so it is left until
//! something actually encodes with B-frames.
//!
//! # Fragments: the caller flushes
//!
//! Samples of every track are buffered until [`Writer::flush_fragment`],
//! which writes one `moof` + `mdat` holding all of them. The intended cadence
//! is **one fragment per keyframe interval** (the 2 s GOP): call
//! `flush_fragment()` immediately before writing each video keyframe after
//! the first, so every fragment opens with a keyframe and a killed process
//! loses at most one GOP. A buffered fragment is capped at
//! [`MAX_FRAGMENT_BYTES`], which only a caller that never flushes reaches.
//!
//! # Crash safety
//!
//! After `flush_fragment()` returns, the file on disk is a complete, playable
//! fragmented MP4: `ftyp`, `moov`, and whole fragments. The fragment is
//! assembled in memory and handed to the OS in one `write_all` on an
//! unbuffered `File`, so it survives the **process** being killed, which is
//! the failure the capture worker exists to contain.
//!
//! It is **not** `sync_data`'d per fragment. An fsync every two seconds
//! protects only against power loss or an OS crash, costs a disk flush on
//! the recording path while the game is running, and still could not make a
//! partly-written tail useful. [`Writer::finish`] does `sync_all` once.
//! After a power loss, [`repair`] recovers what reached the disk; a range the
//! filesystem zero-filled ends the walk at that point.
//!
//! A file with no `mfra` still plays; it just seeks by scanning. [`repair`]
//! truncates a killed file to its last complete fragment and appends one.

// The writer's caller is the own backend's mux (`recorder::own::mux`), which
// runs on Windows only; everywhere else only `repair` has a caller
// (`db::reconcile`'s recovery), and clippy runs without `--all-targets`.
#![cfg_attr(not(any(test, target_os = "windows")), allow(dead_code))]

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

/// The video track's timescale: 90 kHz. See the module header.
pub const VIDEO_TIMESCALE: u32 = 90_000;

/// The movie timescale, used by `mvhd` and `mehd`: milliseconds.
pub const MOVIE_TIMESCALE: u32 = 1_000;

/// The most one fragment may buffer before `write_sample` refuses. Two
/// seconds of 1080p60 at a generous 50 Mbit/s is about 12 MiB, so reaching
/// this means `flush_fragment` is not being called.
pub const MAX_FRAGMENT_BYTES: usize = 512 * 1024 * 1024;

/// `sample_depends_on = 2`: a sync sample, decodable on its own.
const SYNC_SAMPLE: u32 = 0x0200_0000;
/// `sample_depends_on = 1` and `sample_is_non_sync_sample`.
const NON_SYNC_SAMPLE: u32 = 0x0101_0000;

/// `tfhd`: sample data offsets are relative to the start of the `moof`.
const TFHD_DEFAULT_BASE_IS_MOOF: u32 = 0x02_0000;

const TRUN_DATA_OFFSET: u32 = 0x001;
const TRUN_FIRST_SAMPLE_FLAGS: u32 = 0x004;
const TRUN_DURATION: u32 = 0x100;
const TRUN_SIZE: u32 = 0x200;
const TRUN_FLAGS: u32 = 0x400;
const TRUN_CTO: u32 = 0x800;

fn invalid_input(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

fn invalid_data(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

// ---------------------------------------------------------------------------
// H.264: Annex B, AVCC and the SPS
// ---------------------------------------------------------------------------

/// The NAL units of an Annex B byte stream, without their start codes.
///
/// Accepts three- and four-byte start codes, and drops the zero bytes that
/// pad a NAL before the next start code. A NAL cannot legally end in `0x00`
/// (its RBSP ends with a stop bit, and `cabac_zero_word` ends in `0x03`), so
/// that strip never eats payload. Bytes before the first start code are not a
/// NAL and are ignored.
pub fn annex_b_nals(stream: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= stream.len() {
        if stream[i] == 0 && stream[i + 1] == 0 && stream[i + 2] == 1 {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut nals = Vec::with_capacity(starts.len());
    for (n, &start) in starts.iter().enumerate() {
        // The next start code's `00 00 01` begins three bytes before its
        // payload; any extra leading zero is trailing padding, stripped below.
        let end = starts.get(n + 1).map_or(stream.len(), |&next| next - 3);
        let mut nal = &stream[start..end];
        while let [rest @ .., 0] = nal {
            nal = rest;
        }
        if !nal.is_empty() {
            nals.push(nal);
        }
    }
    nals
}

/// Annex B to AVCC: each NAL prefixed with its length as a big-endian `u32`,
/// which is what an `avc1` sample holds (`avcC`'s `lengthSizeMinusOne = 3`).
///
/// Every NAL is kept. [`Writer::write_sample`] additionally drops access unit
/// delimiters and the parameter sets `avcC` already carries.
pub fn annex_b_to_avcc(stream: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(stream.len() + 16);
    for nal in annex_b_nals(stream) {
        out.extend_from_slice(&(nal.len() as u32).to_be_bytes());
        out.extend_from_slice(nal);
    }
    out
}

/// The NAL's `nal_unit_type`.
fn nal_type(nal: &[u8]) -> u8 {
    nal.first().map_or(0, |b| b & 0x1F)
}

const NAL_IDR: u8 = 5;
const NAL_SPS: u8 = 7;
const NAL_PPS: u8 = 8;
const NAL_AUD: u8 = 9;

/// The first SPS and PPS in an Annex B access unit, which is where an H.264
/// encoder puts them ahead of its first keyframe.
pub fn extract_parameter_sets(access_unit: &[u8]) -> Option<(&[u8], &[u8])> {
    let nals = annex_b_nals(access_unit);
    let sps = nals.iter().find(|n| nal_type(n) == NAL_SPS)?;
    let pps = nals.iter().find(|n| nal_type(n) == NAL_PPS)?;
    Some((sps, pps))
}

/// What `avcC` and `tkhd`/`avc1` need from a sequence parameter set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpsInfo {
    pub profile_idc: u8,
    /// The byte after `profile_idc`: `constraint_set0..5_flag` and two
    /// reserved bits, copied verbatim into `avcC`'s compatibility byte.
    pub constraint_flags: u8,
    pub level_idc: u8,
    pub chroma_format_idc: u8,
    pub bit_depth_luma: u8,
    pub bit_depth_chroma: u8,
    /// The display size, after the SPS's cropping rectangle.
    pub width: u32,
    pub height: u32,
}

/// The profiles whose SPS carries `chroma_format_idc` and the bit depths
/// (ITU-T H.264 §7.3.2.1.1), and whose `avcC` must repeat them.
fn is_high_profile(profile_idc: u8) -> bool {
    matches!(
        profile_idc,
        100 | 110 | 122 | 244 | 44 | 83 | 86 | 118 | 128 | 138 | 139 | 134 | 135
    )
}

/// A NAL's payload with emulation-prevention bytes removed: every
/// `00 00 03` becomes `00 00`.
fn rbsp(nal_payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(nal_payload.len());
    let mut zeros = 0;
    for &b in nal_payload {
        if zeros >= 2 && b == 3 {
            zeros = 0;
            continue;
        }
        zeros = if b == 0 { zeros + 1 } else { 0 };
        out.push(b);
    }
    out
}

/// An MSB-first bit reader with the H.264 Exp-Golomb codes.
struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Bits<'_> {
    fn bit(&mut self) -> io::Result<u32> {
        let byte = self
            .data
            .get(self.pos / 8)
            .ok_or_else(|| invalid_data("SPS ends early"))?;
        let bit = (byte >> (7 - self.pos % 8)) & 1;
        self.pos += 1;
        Ok(u32::from(bit))
    }

    fn bits(&mut self, n: u32) -> io::Result<u32> {
        let mut v = 0;
        for _ in 0..n {
            v = (v << 1) | self.bit()?;
        }
        Ok(v)
    }

    fn ue(&mut self) -> io::Result<u32> {
        let mut zeros = 0;
        while self.bit()? == 0 {
            zeros += 1;
            if zeros > 31 {
                return Err(invalid_data("SPS has an Exp-Golomb code over 32 bits"));
            }
        }
        Ok(((1u64 << zeros) - 1 + u64::from(self.bits(zeros)?)) as u32)
    }

    fn se(&mut self) -> io::Result<i32> {
        let k = i64::from(self.ue()?);
        Ok(if k % 2 == 1 { (k + 1) / 2 } else { -(k / 2) } as i32)
    }
}

/// Parse the fields `avcC` and the sample entry need out of an SPS NAL (with
/// its one-byte NAL header, without a start code).
pub fn parse_sps(sps: &[u8]) -> io::Result<SpsInfo> {
    if nal_type(sps) != NAL_SPS || sps.len() < 4 {
        return Err(invalid_data("not an SPS NAL unit"));
    }
    let data = rbsp(&sps[1..]);
    let mut r = Bits {
        data: &data,
        pos: 0,
    };
    let profile_idc = r.bits(8)? as u8;
    let constraint_flags = r.bits(8)? as u8;
    let level_idc = r.bits(8)? as u8;
    let _sps_id = r.ue()?;

    let (mut chroma_format_idc, mut separate_colour_plane) = (1, false);
    let (mut bit_depth_luma, mut bit_depth_chroma) = (8, 8);
    if is_high_profile(profile_idc) {
        chroma_format_idc = r.ue()?;
        if chroma_format_idc > 3 {
            return Err(invalid_data("SPS chroma_format_idc out of range"));
        }
        if chroma_format_idc == 3 {
            separate_colour_plane = r.bit()? == 1;
        }
        bit_depth_luma = r.ue()? + 8;
        bit_depth_chroma = r.ue()? + 8;
        if bit_depth_luma > 14 || bit_depth_chroma > 14 {
            return Err(invalid_data("SPS bit depth out of range"));
        }
        let _qpprime_y_zero_transform_bypass = r.bit()?;
        if r.bit()? == 1 {
            // seq_scaling_matrix_present_flag: skip the lists (§7.3.2.1.1.1).
            let lists = if chroma_format_idc == 3 { 12 } else { 8 };
            for i in 0..lists {
                if r.bit()? == 1 {
                    let size = if i < 6 { 16 } else { 64 };
                    let (mut last, mut next) = (8i32, 8i32);
                    for _ in 0..size {
                        if next != 0 {
                            next = (last + r.se()? + 256).rem_euclid(256);
                        }
                        if next != 0 {
                            last = next;
                        }
                    }
                }
            }
        }
    }

    let _log2_max_frame_num = r.ue()?;
    match r.ue()? {
        0 => {
            let _log2_max_pic_order_cnt_lsb = r.ue()?;
        }
        1 => {
            let _delta_pic_order_always_zero = r.bit()?;
            let _offset_for_non_ref_pic = r.se()?;
            let _offset_for_top_to_bottom_field = r.se()?;
            let cycle = r.ue()?;
            if cycle > 255 {
                return Err(invalid_data("SPS POC cycle out of range"));
            }
            for _ in 0..cycle {
                r.se()?;
            }
        }
        2 => {}
        _ => return Err(invalid_data("SPS pic_order_cnt_type out of range")),
    }
    let _max_num_ref_frames = r.ue()?;
    let _gaps_in_frame_num_allowed = r.bit()?;
    let width_mbs = r.ue()? + 1;
    let height_map_units = r.ue()? + 1;
    let frame_mbs_only = r.bit()?;
    if frame_mbs_only == 0 {
        let _mb_adaptive_frame_field = r.bit()?;
    }
    let _direct_8x8_inference = r.bit()?;
    let (mut crop_left, mut crop_right, mut crop_top, mut crop_bottom) = (0, 0, 0, 0);
    if r.bit()? == 1 {
        crop_left = r.ue()?;
        crop_right = r.ue()?;
        crop_top = r.ue()?;
        crop_bottom = r.ue()?;
    }

    // §7.4.2.1.1: ChromaArrayType, and the crop units that follow from it.
    let chroma_array_type = if separate_colour_plane {
        0
    } else {
        chroma_format_idc
    };
    let (sub_width, sub_height): (u32, u32) = match chroma_array_type {
        1 => (2, 2),
        2 => (2, 1),
        _ => (1, 1),
    };
    let (crop_unit_x, crop_unit_y) = if chroma_array_type == 0 {
        (1, 2 - frame_mbs_only)
    } else {
        (sub_width, sub_height * (2 - frame_mbs_only))
    };
    let full_width = u64::from(width_mbs) * 16;
    let full_height = u64::from(2 - frame_mbs_only) * u64::from(height_map_units) * 16;
    let crop_x = u64::from(crop_unit_x) * (u64::from(crop_left) + u64::from(crop_right));
    let crop_y = u64::from(crop_unit_y) * (u64::from(crop_top) + u64::from(crop_bottom));
    if crop_x >= full_width || crop_y >= full_height || full_width > 65_535 || full_height > 65_535
    {
        return Err(invalid_data("SPS picture size out of range"));
    }

    Ok(SpsInfo {
        profile_idc,
        constraint_flags,
        level_idc,
        chroma_format_idc: chroma_format_idc as u8,
        bit_depth_luma: bit_depth_luma as u8,
        bit_depth_chroma: bit_depth_chroma as u8,
        width: (full_width - crop_x) as u32,
        height: (full_height - crop_y) as u32,
    })
}

// ---------------------------------------------------------------------------
// AAC
// ---------------------------------------------------------------------------

/// ISO/IEC 14496-3 Table 1.18. A rate not listed is written with the escape
/// index and an explicit 24-bit rate.
const AAC_SAMPLE_RATES: [u32; 13] = [
    96_000, 88_200, 64_000, 48_000, 44_100, 32_000, 24_000, 22_050, 16_000, 12_000, 11_025, 8_000,
    7_350,
];

/// The AudioSpecificConfig for AAC-LC (ISO/IEC 14496-3 §1.6.2.1), which
/// `esds` carries as its DecoderSpecificInfo.
///
/// `audioObjectType` 2, the sampling frequency index (or `0xF` and the rate),
/// the channel configuration, then GASpecificConfig's three zero bits: 1024
/// sample frames, no core coder, no extension. Two bytes for every listed
/// rate.
pub fn audio_specific_config(sample_rate: u32, channels: u16) -> io::Result<Vec<u8>> {
    // Channel configurations 1-6 are that many channels; 7 is 7.1 (8).
    let channel_config: u64 = match channels {
        1..=6 => u64::from(channels),
        8 => 7,
        _ => {
            return Err(invalid_input(format!(
                "AAC-LC cannot describe {channels} channels"
            )));
        }
    };
    if sample_rate == 0 || sample_rate >= 1 << 24 {
        return Err(invalid_input(format!(
            "AAC sample rate {sample_rate} out of range"
        )));
    }
    let (mut bits, mut len): (u64, u32) = (2, 5);
    let mut push = |value: u64, width: u32| {
        bits = (bits << width) | value;
        len += width;
    };
    match AAC_SAMPLE_RATES.iter().position(|&r| r == sample_rate) {
        Some(index) => push(index as u64, 4),
        None => {
            push(0xF, 4);
            push(u64::from(sample_rate), 24);
        }
    }
    push(channel_config, 4);
    push(0, 3);
    let bytes = len.div_ceil(8);
    bits <<= bytes * 8 - len;
    Ok((0..bytes).rev().map(|i| (bits >> (i * 8)) as u8).collect())
}

/// An MPEG-4 descriptor length (ISO/IEC 14496-1 §8.3.3): seven bits per
/// byte, most significant first, the top bit set on all but the last.
fn push_descriptor_len(out: &mut Vec<u8>, len: usize) {
    debug_assert!(len < 1 << 28);
    let mut groups = vec![(len & 0x7F) as u8];
    let mut rest = len >> 7;
    while rest > 0 {
        groups.push((rest & 0x7F) as u8 | 0x80);
        rest >>= 7;
    }
    out.extend(groups.iter().rev());
}

fn push_descriptor(out: &mut Vec<u8>, tag: u8, body: &[u8]) {
    out.push(tag);
    push_descriptor_len(out, body.len());
    out.extend_from_slice(body);
}

/// The body of an `esds` box after its version and flags: an ES_Descriptor
/// holding a DecoderConfigDescriptor (MPEG-4 audio, AudioStream) with the
/// AudioSpecificConfig, and the MP4 file format's fixed SLConfigDescriptor.
fn es_descriptor(es_id: u16, asc: &[u8]) -> Vec<u8> {
    let mut specific = Vec::new();
    push_descriptor(&mut specific, 0x05, asc);

    let mut config = vec![
        0x40, // objectTypeIndication: MPEG-4 audio (ISO/IEC 14496-3)
        0x15, // streamType 5 (audio) << 2, upStream 0, reserved 1
        0, 0, 0, // bufferSizeDB: unknown
    ];
    config.extend_from_slice(&0u32.to_be_bytes()); // maxBitrate: unknown
    config.extend_from_slice(&0u32.to_be_bytes()); // avgBitrate: variable
    config.extend_from_slice(&specific);

    let mut es = es_id.to_be_bytes().to_vec();
    es.push(0); // no stream dependence, URL or OCR stream
    push_descriptor(&mut es, 0x04, &config);
    push_descriptor(&mut es, 0x06, &[0x02]); // SLConfigDescriptor, predefined = MP4

    let mut out = Vec::new();
    push_descriptor(&mut out, 0x03, &es);
    out
}

// ---------------------------------------------------------------------------
// Tracks
// ---------------------------------------------------------------------------

/// One track of the file, as declared to [`Writer::create`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Track {
    /// H.264, from its SPS and PPS (NAL units without start codes).
    H264 {
        sps: Vec<u8>,
        pps: Vec<u8>,
        info: SpsInfo,
    },
    /// AAC-LC.
    Aac { sample_rate: u32, channels: u16 },
}

impl Track {
    /// An H.264 track. The SPS is parsed here, so a bad one fails at create
    /// rather than producing a file nothing can decode.
    pub fn h264(sps: &[u8], pps: &[u8]) -> io::Result<Self> {
        let info = parse_sps(sps)?;
        if nal_type(pps) != NAL_PPS {
            return Err(invalid_input("not a PPS NAL unit"));
        }
        if sps.len() > usize::from(u16::MAX) || pps.len() > usize::from(u16::MAX) {
            return Err(invalid_input("parameter set too long for avcC"));
        }
        Ok(Track::H264 {
            sps: sps.to_vec(),
            pps: pps.to_vec(),
            info,
        })
    }

    /// An H.264 track from the encoder's first keyframe, which carries the
    /// SPS and PPS in-band.
    pub fn h264_from_annex_b(access_unit: &[u8]) -> io::Result<Self> {
        let (sps, pps) = extract_parameter_sets(access_unit)
            .ok_or_else(|| invalid_input("access unit has no SPS and PPS"))?;
        Track::h264(sps, pps)
    }

    /// An AAC-LC track. Samples are raw access units: no ADTS header.
    pub fn aac_lc(sample_rate: u32, channels: u16) -> io::Result<Self> {
        audio_specific_config(sample_rate, channels)?;
        Ok(Track::Aac {
            sample_rate,
            channels,
        })
    }

    /// The unit of every `pts` and `duration` passed for this track.
    pub fn timescale(&self) -> u32 {
        match self {
            Track::H264 { .. } => VIDEO_TIMESCALE,
            Track::Aac { sample_rate, .. } => *sample_rate,
        }
    }

    fn is_video(&self) -> bool {
        matches!(self, Track::H264 { .. })
    }
}

// ---------------------------------------------------------------------------
// Box building
// ---------------------------------------------------------------------------

fn p8(out: &mut Vec<u8>, v: u8) {
    out.push(v);
}
fn p16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_be_bytes());
}
fn p32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_be_bytes());
}
fn p64(out: &mut Vec<u8>, v: u64) {
    out.extend_from_slice(&v.to_be_bytes());
}

/// A box: its size is patched in once `body` has written it.
///
/// Only `moov`, `moof` and `mfra` are built this way, and all three are
/// bounded by metadata (the fragment cap keeps a `moof` far below 4 GiB), so
/// the 32-bit size cannot overflow. `mdat` is written by hand.
fn bx(out: &mut Vec<u8>, kind: &[u8; 4], body: impl FnOnce(&mut Vec<u8>)) {
    let start = out.len();
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(kind);
    body(out);
    let size = (out.len() - start) as u32;
    out[start..start + 4].copy_from_slice(&size.to_be_bytes());
}

/// A full box: a box whose body opens with a version byte and 24 flag bits.
fn full(
    out: &mut Vec<u8>,
    kind: &[u8; 4],
    version: u8,
    flags: u32,
    body: impl FnOnce(&mut Vec<u8>),
) {
    bx(out, kind, |o| {
        p8(o, version);
        o.extend_from_slice(&flags.to_be_bytes()[1..]);
        body(o);
    });
}

const UNITY_MATRIX: [u32; 9] = [0x0001_0000, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000];

fn ftyp() -> Vec<u8> {
    let mut out = Vec::new();
    bx(&mut out, b"ftyp", |o| {
        o.extend_from_slice(b"isom");
        p32(o, 0x200);
        for brand in [b"isom", b"iso6", b"iso2", b"avc1", b"mp41"] {
            o.extend_from_slice(brand);
        }
    });
    out
}

/// `moov`, and the offset within it of `mehd`'s 64-bit duration, which
/// `finish` and `repair` patch once the length is known.
fn moov(tracks: &[Track]) -> (Vec<u8>, usize) {
    let mut out = Vec::new();
    let mut mehd_value_at = 0;
    bx(&mut out, b"moov", |o| {
        full(o, b"mvhd", 0, 0, |o| {
            p32(o, 0); // creation_time
            p32(o, 0); // modification_time
            p32(o, MOVIE_TIMESCALE);
            p32(o, 0); // duration: fragmented, so mehd and the fragments say
            p32(o, 0x0001_0000); // rate 1.0
            p16(o, 0x0100); // volume 1.0
            o.extend_from_slice(&[0; 10]);
            UNITY_MATRIX.iter().for_each(|&m| p32(o, m));
            o.extend_from_slice(&[0; 24]); // pre_defined
            p32(o, tracks.len() as u32 + 1); // next_track_ID
        });
        let mut first_audio = true;
        for (index, track) in tracks.iter().enumerate() {
            // Track 0 of the audio is the combined mix and the only one a
            // plain player should pick (DEVELOPMENT.md §2.5): it is enabled,
            // the stems are not, and all audio shares alternate group 1.
            // ffmpeg reads "enabled" as the default disposition, which is
            // what the libobs faststart remux sets on track 0.
            let enabled = track.is_video() || std::mem::take(&mut first_audio);
            trak(o, index as u32 + 1, track, enabled);
        }
        bx(o, b"mvex", |o| {
            full(o, b"mehd", 1, 0, |o| {
                mehd_value_at = o.len();
                p64(o, 0);
            });
            for index in 0..tracks.len() {
                full(o, b"trex", 0, 0, |o| {
                    p32(o, index as u32 + 1);
                    p32(o, 1); // default_sample_description_index
                    p32(o, 0); // default_sample_duration
                    p32(o, 0); // default_sample_size
                    p32(o, 0); // default_sample_flags
                });
            }
        });
    });
    (out, mehd_value_at)
}

fn trak(o: &mut Vec<u8>, track_id: u32, track: &Track, enabled: bool) {
    let (width, height) = match track {
        Track::H264 { info, .. } => (info.width, info.height),
        Track::Aac { .. } => (0, 0),
    };
    bx(o, b"trak", |o| {
        // flags: track_enabled 0x1, track_in_movie 0x2.
        full(o, b"tkhd", 0, if enabled { 0x3 } else { 0x2 }, |o| {
            p32(o, 0); // creation_time
            p32(o, 0); // modification_time
            p32(o, track_id);
            p32(o, 0);
            p32(o, 0); // duration
            o.extend_from_slice(&[0; 8]);
            p16(o, 0); // layer
            p16(o, if track.is_video() { 0 } else { 1 }); // alternate_group
            p16(o, if track.is_video() { 0 } else { 0x0100 }); // volume
            p16(o, 0);
            UNITY_MATRIX.iter().for_each(|&m| p32(o, m));
            p32(o, width << 16);
            p32(o, height << 16);
        });
        bx(o, b"mdia", |o| {
            full(o, b"mdhd", 0, 0, |o| {
                p32(o, 0);
                p32(o, 0);
                p32(o, track.timescale());
                p32(o, 0); // duration
                p16(o, 0x55C4); // language: "und", packed ISO-639-2/T
                p16(o, 0);
            });
            full(o, b"hdlr", 0, 0, |o| {
                p32(o, 0);
                o.extend_from_slice(if track.is_video() { b"vide" } else { b"soun" });
                o.extend_from_slice(&[0; 12]);
                let name: &[u8] = if track.is_video() {
                    b"VideoHandler\0"
                } else {
                    b"SoundHandler\0"
                };
                o.extend_from_slice(name);
            });
            bx(o, b"minf", |o| {
                if track.is_video() {
                    full(o, b"vmhd", 0, 1, |o| o.extend_from_slice(&[0; 8]));
                } else {
                    full(o, b"smhd", 0, 0, |o| p32(o, 0));
                }
                bx(o, b"dinf", |o| {
                    full(o, b"dref", 0, 0, |o| {
                        p32(o, 1);
                        // flags 1: the media is in this file.
                        full(o, b"url ", 0, 1, |_| {});
                    });
                });
                bx(o, b"stbl", |o| {
                    full(o, b"stsd", 0, 0, |o| {
                        p32(o, 1);
                        sample_entry(o, track_id, track);
                    });
                    full(o, b"stts", 0, 0, |o| p32(o, 0));
                    full(o, b"stsc", 0, 0, |o| p32(o, 0));
                    full(o, b"stsz", 0, 0, |o| {
                        p32(o, 0);
                        p32(o, 0);
                    });
                    full(o, b"stco", 0, 0, |o| p32(o, 0));
                });
            });
        });
    });
}

fn sample_entry(o: &mut Vec<u8>, track_id: u32, track: &Track) {
    match track {
        Track::H264 { sps, pps, info } => bx(o, b"avc1", |o| {
            o.extend_from_slice(&[0; 6]);
            p16(o, 1); // data_reference_index
            o.extend_from_slice(&[0; 16]); // pre_defined, reserved
            p16(o, info.width as u16);
            p16(o, info.height as u16);
            p32(o, 0x0048_0000); // 72 dpi
            p32(o, 0x0048_0000);
            p32(o, 0);
            p16(o, 1); // frame_count
            o.extend_from_slice(&[0; 32]); // compressorname
            p16(o, 0x0018); // depth
            p16(o, 0xFFFF); // pre_defined = -1
            bx(o, b"avcC", |o| avc_config(o, sps, pps, info));
        }),
        Track::Aac {
            sample_rate,
            channels,
        } => bx(o, b"mp4a", |o| {
            o.extend_from_slice(&[0; 6]);
            p16(o, 1); // data_reference_index
            o.extend_from_slice(&[0; 8]);
            p16(o, *channels);
            p16(o, 16); // samplesize
            p32(o, 0);
            // 16.16 fixed point, so a rate above 65535 does not fit; the
            // AudioSpecificConfig is authoritative either way.
            p32(
                o,
                if *sample_rate <= 0xFFFF {
                    sample_rate << 16
                } else {
                    0
                },
            );
            let asc =
                audio_specific_config(*sample_rate, *channels).expect("validated by Track::aac_lc");
            full(o, b"esds", 0, 0, |o| {
                o.extend_from_slice(&es_descriptor(track_id as u16, &asc))
            });
        }),
    }
}

/// The AVCDecoderConfigurationRecord (ISO/IEC 14496-15 §5.3.3.1).
fn avc_config(o: &mut Vec<u8>, sps: &[u8], pps: &[u8], info: &SpsInfo) {
    p8(o, 1); // configurationVersion
    p8(o, info.profile_idc);
    p8(o, info.constraint_flags);
    p8(o, info.level_idc);
    p8(o, 0xFC | 3); // lengthSizeMinusOne = 3
    p8(o, 0xE0 | 1); // one SPS
    p16(o, sps.len() as u16);
    o.extend_from_slice(sps);
    p8(o, 1); // one PPS
    p16(o, pps.len() as u16);
    o.extend_from_slice(pps);
    if matches!(info.profile_idc, 100 | 110 | 122 | 144) {
        p8(o, 0xFC | info.chroma_format_idc);
        p8(o, 0xF8 | (info.bit_depth_luma - 8));
        p8(o, 0xF8 | (info.bit_depth_chroma - 8));
        p8(o, 0); // numOfSequenceParameterSetExt
    }
}

/// One `tfra` entry: a fragment whose run for this track opens on a sync
/// sample.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RandomAccess {
    /// Presentation time of that sample, in the track's timescale.
    time: u64,
    moof_offset: u64,
    /// 1-based position of the track's `traf` within the `moof`.
    traf_number: u8,
}

fn mfra(tracks: &[(u32, Vec<RandomAccess>)]) -> Vec<u8> {
    let mut out = Vec::new();
    bx(&mut out, b"mfra", |o| {
        for (track_id, entries) in tracks {
            full(o, b"tfra", 1, 0, |o| {
                p32(o, *track_id);
                // length_size_of_traf_num, trun_num and sample_num: 1 byte each.
                p32(o, 0);
                p32(o, entries.len() as u32);
                for e in entries {
                    p64(o, e.time);
                    p64(o, e.moof_offset);
                    p8(o, e.traf_number);
                    p8(o, 1); // trun_number
                    p8(o, 1); // sample_number
                }
            });
        }
        let size = o.len() + 16; // this mfro included
        full(o, b"mfro", 0, 0, |o| p32(o, size as u32));
    });
    out
}

/// The movie duration in `mehd`'s milliseconds, rounded up, from each
/// track's end time in its own timescale.
fn movie_duration(ends: impl Iterator<Item = (u64, u32)>) -> u64 {
    ends.map(|(end, scale)| {
        (u128::from(end) * u128::from(MOVIE_TIMESCALE)).div_ceil(u128::from(scale)) as u64
    })
    .max()
    .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// The writer
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
struct SampleMeta {
    size: u32,
    duration: u32,
    flags: u32,
    composition_offset: i32,
}

struct TrackState {
    track: Track,
    /// The decode time the next sample gets; `None` before the first.
    next_dts: Option<u64>,
    /// The decode time of the buffered fragment's first sample.
    fragment_dts: u64,
    samples: Vec<SampleMeta>,
    data: Vec<u8>,
    random_access: Vec<RandomAccess>,
}

/// A fragmented MP4 being written. See the module header for the layout,
/// the timescales and what is on disk when.
///
/// Dropping it without [`finish`](Writer::finish) leaves every flushed
/// fragment on disk and no `mfra`, which is the state [`repair`] fixes.
/// Samples buffered since the last flush are lost.
pub struct Writer {
    file: File,
    tracks: Vec<TrackState>,
    /// Bytes written so far, which is where the next `moof` starts.
    position: u64,
    /// Absolute offset of `mehd`'s duration.
    mehd_value_at: u64,
    sequence: u32,
    /// Set when a write failed part-way: the tail on disk is torn, and
    /// appending after it would produce a file `repair` stops short of.
    broken: bool,
}

impl Writer {
    /// Create (or truncate) `path` and write `ftyp` and `moov`, so the file
    /// is a valid, empty fragmented MP4 from the moment this returns.
    ///
    /// Track ids are the index plus one, in the order given. Convention is
    /// video first, then the audio tracks in `AudioPreset::layout` order.
    pub fn create(path: &Path, tracks: Vec<Track>) -> io::Result<Self> {
        if tracks.is_empty() {
            return Err(invalid_input("an MP4 needs at least one track"));
        }
        if tracks.len() > usize::from(u8::MAX) {
            return Err(invalid_input(
                "too many tracks for tfra's one-byte traf number",
            ));
        }
        let mut header = ftyp();
        let (moov, mehd_in_moov) = moov(&tracks);
        let mehd_value_at = (header.len() + mehd_in_moov) as u64;
        header.extend_from_slice(&moov);

        let mut file = File::create(path)?;
        file.write_all(&header)?;
        Ok(Writer {
            file,
            position: header.len() as u64,
            mehd_value_at,
            sequence: 0,
            broken: false,
            tracks: tracks
                .into_iter()
                .map(|track| TrackState {
                    track,
                    next_dts: None,
                    fragment_dts: 0,
                    samples: Vec::new(),
                    data: Vec::new(),
                    random_access: Vec::new(),
                })
                .collect(),
        })
    }

    /// Buffer one sample for the current fragment.
    ///
    /// `track` is the index into the tracks given to `create`; `pts` and
    /// `duration` are in that track's [`timescale`](Track::timescale), in
    /// decode order (see the module header on how the decode time follows
    /// from them).
    ///
    /// - **H.264:** `bytes` is one access unit in Annex B, as the encoder
    ///   emits it. It is converted to AVCC; access unit delimiters and the
    ///   parameter sets are dropped, and a parameter set that differs from
    ///   the one the track was created with is refused, since `avc1` has no
    ///   way to change it mid-file. The track's first sample must be a
    ///   keyframe.
    /// - **AAC:** `bytes` is one raw access unit without an ADTS header.
    ///   Every AAC frame is a sync sample, so `keyframe` is ignored.
    pub fn write_sample(
        &mut self,
        track: usize,
        pts: u64,
        duration: u32,
        keyframe: bool,
        bytes: &[u8],
    ) -> io::Result<()> {
        if self.broken {
            return Err(io::Error::other(
                "an earlier write failed; the file needs repair",
            ));
        }
        let state = self
            .tracks
            .get_mut(track)
            .ok_or_else(|| invalid_input(format!("no track {track}")))?;
        if duration == 0 {
            return Err(invalid_input("a sample needs a non-zero duration"));
        }
        let video = state.track.is_video();
        if video && state.next_dts.is_none() && !keyframe {
            return Err(invalid_input("a video track must start with a keyframe"));
        }
        if state.data.len() + bytes.len() > MAX_FRAGMENT_BYTES {
            return Err(invalid_input(
                "fragment too large: flush_fragment is not being called",
            ));
        }
        let dts = state.next_dts.unwrap_or(pts);
        let composition_offset =
            i32::try_from(i128::from(pts) - i128::from(dts)).map_err(|_| {
                invalid_input("pts is too far from the decode time its durations imply")
            })?;
        let next_dts = dts
            .checked_add(u64::from(duration))
            .ok_or_else(|| invalid_input("decode time overflows"))?;

        let start = state.data.len();
        let appended = match &state.track {
            Track::H264 { sps, pps, .. } => push_avc_sample(&mut state.data, bytes, sps, pps),
            Track::Aac { .. } => {
                state.data.extend_from_slice(bytes);
                Ok(())
            }
        };
        if let Err(e) = appended {
            state.data.truncate(start);
            return Err(e);
        }
        let size = state.data.len() - start;
        if size == 0 {
            return Err(invalid_input("an empty sample"));
        }

        if state.samples.is_empty() {
            state.fragment_dts = dts;
        }
        state.samples.push(SampleMeta {
            size: size as u32,
            duration,
            flags: if !video || keyframe {
                SYNC_SAMPLE
            } else {
                NON_SYNC_SAMPLE
            },
            composition_offset,
        });
        state.next_dts = Some(next_dts);
        Ok(())
    }

    /// Write every buffered sample as one `moof` + `mdat`, and hand it to
    /// the OS. A no-op when nothing is buffered.
    ///
    /// Call it immediately before each video keyframe after the first; see
    /// the module header.
    pub fn flush_fragment(&mut self) -> io::Result<()> {
        if self.broken {
            return Err(io::Error::other(
                "an earlier write failed; the file needs repair",
            ));
        }
        if self.tracks.iter().all(|t| t.samples.is_empty()) {
            return Ok(());
        }
        self.sequence += 1;
        let moof_offset = self.position;
        let (mut buf, data_offset_at) = moof(self.sequence, &self.tracks);

        let payload: usize = self.tracks.iter().map(|t| t.data.len()).sum();
        let mdat_size = u32::try_from(payload + 8)
            .map_err(|_| invalid_input("fragment too large for a 32-bit mdat"))?;
        // Each traf's data starts after the moof, the mdat header and the
        // tracks before it.
        let mut offset = buf.len() + 8;
        let active = self.tracks.iter().filter(|t| !t.samples.is_empty());
        for (&at, state) in data_offset_at.iter().zip(active) {
            buf[at..at + 4].copy_from_slice(&(offset as u32).to_be_bytes());
            offset += state.data.len();
        }
        buf.reserve(payload + 8);
        p32(&mut buf, mdat_size);
        buf.extend_from_slice(b"mdat");
        for state in &self.tracks {
            buf.extend_from_slice(&state.data);
        }

        if let Err(e) = self.file.write_all(&buf).and_then(|()| self.file.flush()) {
            self.broken = true;
            return Err(e);
        }
        self.position += buf.len() as u64;

        let active = self.tracks.iter_mut().filter(|t| !t.samples.is_empty());
        for (index, state) in active.enumerate() {
            // At most 255 tracks, checked by `create`.
            let traf_number = index as u8 + 1;
            let first = state.samples[0];
            if first.flags == SYNC_SAMPLE {
                state.random_access.push(RandomAccess {
                    time: state
                        .fragment_dts
                        .saturating_add_signed(i64::from(first.composition_offset)),
                    moof_offset,
                    traf_number,
                });
            }
            state.samples.clear();
            state.data.clear();
        }
        Ok(())
    }

    /// Flush the last fragment, append the `mfra`, fill in `mehd`'s
    /// duration, and `sync_all`.
    pub fn finish(mut self) -> io::Result<()> {
        self.flush_fragment()?;
        let index: Vec<(u32, Vec<RandomAccess>)> = self
            .tracks
            .iter_mut()
            .enumerate()
            .map(|(i, t)| (i as u32 + 1, std::mem::take(&mut t.random_access)))
            .collect();
        self.file.write_all(&mfra(&index))?;
        let duration = movie_duration(
            self.tracks
                .iter()
                .map(|t| (t.next_dts.unwrap_or(0), t.track.timescale())),
        );
        self.file.seek(SeekFrom::Start(self.mehd_value_at))?;
        self.file.write_all(&duration.to_be_bytes())?;
        self.file.sync_all()
    }
}

/// Append one Annex B access unit to `out` as AVCC, dropping AUDs and the
/// parameter sets (refusing ones that differ from the track's).
fn push_avc_sample(
    out: &mut Vec<u8>,
    access_unit: &[u8],
    sps: &[u8],
    pps: &[u8],
) -> io::Result<()> {
    let nals = annex_b_nals(access_unit);
    if nals.is_empty() {
        return Err(invalid_input(
            "an H.264 sample must be Annex B, with start codes",
        ));
    }
    for nal in nals {
        match nal_type(nal) {
            NAL_AUD => {}
            NAL_SPS if nal == sps => {}
            NAL_PPS if nal == pps => {}
            NAL_SPS | NAL_PPS => {
                return Err(invalid_input(
                    "the stream changed its SPS or PPS; start a new file",
                ));
            }
            _ => {
                out.extend_from_slice(&(nal.len() as u32).to_be_bytes());
                out.extend_from_slice(nal);
            }
        }
    }
    Ok(())
}

/// The `moof` for every track with buffered samples, and the offset within
/// it of each `trun`'s `data_offset`, to be patched once the sizes are known.
fn moof(sequence: u32, tracks: &[TrackState]) -> (Vec<u8>, Vec<usize>) {
    let mut out = Vec::new();
    let mut data_offset_at = Vec::new();
    bx(&mut out, b"moof", |o| {
        full(o, b"mfhd", 0, 0, |o| p32(o, sequence));
        for (index, state) in tracks.iter().enumerate() {
            if state.samples.is_empty() {
                continue;
            }
            bx(o, b"traf", |o| {
                full(o, b"tfhd", 0, TFHD_DEFAULT_BASE_IS_MOOF, |o| {
                    p32(o, index as u32 + 1)
                });
                full(o, b"tfdt", 1, 0, |o| p64(o, state.fragment_dts));
                let negative = state.samples.iter().any(|s| s.composition_offset < 0);
                let any_offset = state.samples.iter().any(|s| s.composition_offset != 0);
                let flags = TRUN_DATA_OFFSET
                    | TRUN_DURATION
                    | TRUN_SIZE
                    | TRUN_FLAGS
                    | if any_offset { TRUN_CTO } else { 0 };
                full(o, b"trun", u8::from(negative), flags, |o| {
                    p32(o, state.samples.len() as u32);
                    data_offset_at.push(o.len());
                    p32(o, 0);
                    for s in &state.samples {
                        p32(o, s.duration);
                        p32(o, s.size);
                        p32(o, s.flags);
                        if any_offset {
                            // Version 0 reads it unsigned; it is only
                            // version 0 when none is negative.
                            p32(o, s.composition_offset as u32);
                        }
                    }
                });
            });
        }
    });
    (out, data_offset_at)
}

// ---------------------------------------------------------------------------
// Repair
// ---------------------------------------------------------------------------

/// What [`repair`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Repaired {
    /// Complete fragments kept.
    pub fragments: usize,
    /// The file's length before any `mfra` was appended.
    pub kept_bytes: u64,
    /// Bytes cut from the end: a torn fragment, or a torn `mfra`.
    pub removed_bytes: u64,
    /// The file already ended in a complete `mfra`; nothing was changed.
    pub already_complete: bool,
}

/// Make a killed recording whole: truncate it to its last complete
/// `moof` + `mdat`, append an `mfra` indexing what is left, fill in `mehd`,
/// and `sync_all`.
///
/// Recovery in Rust with no ffmpeg (WS1.6.7 wires it into
/// `recover_unfinished`). It understands the files [`Writer`] writes, not
/// fragmented MP4 in general: a complete fragment it cannot follow is an
/// error rather than a place to cut, so a file from elsewhere is refused
/// instead of shortened. A file that already ends in a complete `mfra` is
/// left alone.
pub fn repair(path: &Path) -> io::Result<Repaired> {
    let mut file = OpenOptions::new().read(true).write(true).open(path)?;
    let len = file.metadata()?.len();
    let plan = plan_repair(&mut io::BufReader::new(&mut file), len)?;
    let report = Repaired {
        fragments: plan.fragments,
        kept_bytes: plan.keep,
        removed_bytes: len - plan.keep,
        already_complete: plan.already_complete,
    };
    if plan.already_complete {
        return Ok(report);
    }
    file.set_len(plan.keep)?;
    file.seek(SeekFrom::Start(plan.keep))?;
    file.write_all(&plan.mfra)?;
    if let Some(at) = plan.mehd_value_at {
        file.seek(SeekFrom::Start(at))?;
        file.write_all(&plan.duration.to_be_bytes())?;
    }
    file.sync_all()?;
    Ok(report)
}

/// The decision half of [`repair`], reading only box headers and the small
/// boxes (`moov`, `moof`) so a multi-gigabyte file is never loaded.
#[derive(Debug)]
struct RepairPlan {
    keep: u64,
    fragments: usize,
    mfra: Vec<u8>,
    mehd_value_at: Option<u64>,
    duration: u64,
    already_complete: bool,
}

/// The largest `moov` or `moof` [`plan_repair`] will read into memory. Ours
/// are kilobytes; this only stops a corrupt size field allocating gigabytes.
const MAX_METADATA_BOX: u64 = 64 * 1024 * 1024;

struct BoxHeader {
    kind: [u8; 4],
    /// The whole box, header included; 0 means "to the end of the file".
    size: u64,
    header_len: u64,
}

/// The header of the box at `at`, or `None` if the file ends inside it.
fn read_header<R: Read + Seek>(src: &mut R, at: u64, len: u64) -> io::Result<Option<BoxHeader>> {
    if at + 8 > len {
        return Ok(None);
    }
    src.seek(SeekFrom::Start(at))?;
    let mut head = [0u8; 8];
    src.read_exact(&mut head)?;
    let kind = [head[4], head[5], head[6], head[7]];
    let size = u64::from(u32::from_be_bytes([head[0], head[1], head[2], head[3]]));
    if size == 1 {
        if at + 16 > len {
            return Ok(None);
        }
        let mut large = [0u8; 8];
        src.read_exact(&mut large)?;
        return Ok(Some(BoxHeader {
            kind,
            size: u64::from_be_bytes(large),
            header_len: 16,
        }));
    }
    Ok(Some(BoxHeader {
        kind,
        size,
        header_len: 8,
    }))
}

fn read_body<R: Read + Seek>(src: &mut R, at: u64, header: &BoxHeader) -> io::Result<Vec<u8>> {
    if header.size > MAX_METADATA_BOX {
        return Err(invalid_data(format!(
            "{} box of {} bytes is not one this writer produced",
            String::from_utf8_lossy(&header.kind),
            header.size
        )));
    }
    src.seek(SeekFrom::Start(at + header.header_len))?;
    let mut body = vec![0u8; (header.size - header.header_len) as usize];
    src.read_exact(&mut body)?;
    Ok(body)
}

/// A child box: its kind, the offset of its body within the parent's body,
/// and the body.
type Child<'a> = ([u8; 4], usize, &'a [u8]);

/// The child boxes of a box body.
fn children(body: &[u8]) -> io::Result<Vec<Child<'_>>> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < body.len() {
        let head = body
            .get(at..at + 8)
            .ok_or_else(|| invalid_data("box header runs past its parent"))?;
        let size = u32::from_be_bytes([head[0], head[1], head[2], head[3]]) as usize;
        if size < 8 || at + size > body.len() {
            return Err(invalid_data("box size runs past its parent"));
        }
        let kind = [head[4], head[5], head[6], head[7]];
        out.push((kind, at + 8, &body[at + 8..at + size]));
        at += size;
    }
    Ok(out)
}

fn child<'a>(body: &'a [u8], kind: &[u8; 4]) -> io::Result<Option<(usize, &'a [u8])>> {
    Ok(children(body)?
        .into_iter()
        .find(|(k, _, _)| k == kind)
        .map(|(_, at, b)| (at, b)))
}

/// A cursor over a box body, failing rather than panicking at its end.
struct Fields<'a> {
    data: &'a [u8],
    at: usize,
}

impl Fields<'_> {
    fn take<const N: usize>(&mut self) -> io::Result<[u8; N]> {
        let bytes = self
            .data
            .get(self.at..self.at + N)
            .ok_or_else(|| invalid_data("box body ends early"))?;
        self.at += N;
        Ok(bytes.try_into().expect("slice of length N"))
    }
    fn u32(&mut self) -> io::Result<u32> {
        Ok(u32::from_be_bytes(self.take()?))
    }
    fn u64(&mut self) -> io::Result<u64> {
        Ok(u64::from_be_bytes(self.take()?))
    }
    /// Version and flags.
    fn full(&mut self) -> io::Result<(u8, u32)> {
        let v = self.u32()?;
        Ok(((v >> 24) as u8, v & 0x00FF_FFFF))
    }
}

/// A track as `moov` declares it: its id and its timescale.
fn moov_tracks(moov: &[u8]) -> io::Result<Vec<(u32, u32)>> {
    let mut out = Vec::new();
    for (kind, _, trak) in children(moov)? {
        if &kind != b"trak" {
            continue;
        }
        let (_, tkhd) = child(trak, b"tkhd")?.ok_or_else(|| invalid_data("trak without tkhd"))?;
        let mut f = Fields { data: tkhd, at: 0 };
        let (version, _) = f.full()?;
        f.at += if version == 1 { 16 } else { 8 };
        let track_id = f.u32()?;
        let (_, mdia) = child(trak, b"mdia")?.ok_or_else(|| invalid_data("trak without mdia"))?;
        let (_, mdhd) = child(mdia, b"mdhd")?.ok_or_else(|| invalid_data("mdia without mdhd"))?;
        let mut f = Fields { data: mdhd, at: 0 };
        let (version, _) = f.full()?;
        f.at += if version == 1 { 16 } else { 8 };
        let timescale = f.u32()?;
        if timescale == 0 {
            return Err(invalid_data("a track with timescale 0"));
        }
        out.push((track_id, timescale));
    }
    if out.is_empty() {
        return Err(invalid_data("moov declares no tracks"));
    }
    Ok(out)
}

/// One `traf`, as far as repair needs it.
struct TrafSummary {
    track_id: u32,
    /// Decode time just past its last sample.
    end_dts: u64,
    /// Presentation time of its first sample, if that is a sync sample.
    sync_time: Option<u64>,
    /// Its sample data, relative to the start of the `moof`.
    data: std::ops::Range<u64>,
}

fn parse_traf(traf: &[u8]) -> io::Result<TrafSummary> {
    let (_, tfhd) = child(traf, b"tfhd")?.ok_or_else(|| invalid_data("traf without tfhd"))?;
    let mut f = Fields { data: tfhd, at: 0 };
    let (_, tfhd_flags) = f.full()?;
    let track_id = f.u32()?;
    if tfhd_flags & 0x01 != 0 {
        return Err(invalid_data(
            "tfhd with a base_data_offset: not a file this writer produced",
        ));
    }
    if tfhd_flags & 0x02 != 0 {
        f.u32()?;
    }
    let default_duration = if tfhd_flags & 0x08 != 0 {
        Some(f.u32()?)
    } else {
        None
    };
    let default_size = if tfhd_flags & 0x10 != 0 {
        Some(f.u32()?)
    } else {
        None
    };
    // An absent default falls back to trex's, which this writer leaves at 0:
    // a sync sample.
    let default_flags = if tfhd_flags & 0x20 != 0 { f.u32()? } else { 0 };

    let (_, tfdt) = child(traf, b"tfdt")?.ok_or_else(|| invalid_data("traf without tfdt"))?;
    let mut f = Fields { data: tfdt, at: 0 };
    let base_dts = match f.full()?.0 {
        1 => f.u64()?,
        _ => u64::from(f.u32()?),
    };

    let truns: Vec<_> = children(traf)?
        .into_iter()
        .filter(|(k, _, _)| k == b"trun")
        .collect();
    let [(_, _, trun)] = truns.as_slice() else {
        return Err(invalid_data("expected exactly one trun per traf"));
    };
    let mut f = Fields { data: trun, at: 0 };
    let (version, flags) = f.full()?;
    let count = f.u32()?;
    if flags & TRUN_DATA_OFFSET == 0 {
        return Err(invalid_data("trun without a data_offset"));
    }
    let data_offset = f.u32()? as i32;
    let first_flags = if flags & TRUN_FIRST_SAMPLE_FLAGS != 0 {
        Some(f.u32()?)
    } else {
        None
    };

    let mut dts = base_dts;
    let mut bytes = 0u64;
    let mut sync_time = None;
    for i in 0..count {
        let duration = if flags & TRUN_DURATION != 0 {
            f.u32()?
        } else {
            default_duration.unwrap_or(0)
        };
        let size = if flags & TRUN_SIZE != 0 {
            f.u32()?
        } else {
            default_size.ok_or_else(|| invalid_data("trun sample without a size"))?
        };
        let sample_flags = if flags & TRUN_FLAGS != 0 {
            f.u32()?
        } else {
            default_flags
        };
        let offset = if flags & TRUN_CTO != 0 {
            let raw = f.u32()?;
            if version == 0 {
                i64::from(raw)
            } else {
                i64::from(raw as i32)
            }
        } else {
            0
        };
        if i == 0 {
            let first = first_flags.unwrap_or(sample_flags);
            // sample_is_non_sync_sample is bit 16.
            if first & 0x0001_0000 == 0 {
                sync_time = Some(dts.saturating_add_signed(offset));
            }
        }
        dts += u64::from(duration);
        bytes += u64::from(size);
    }
    let start =
        u64::try_from(data_offset).map_err(|_| invalid_data("negative trun data_offset"))?;
    Ok(TrafSummary {
        track_id,
        end_dts: dts,
        sync_time,
        data: start..start + bytes,
    })
}

fn plan_repair<R: Read + Seek>(src: &mut R, len: u64) -> io::Result<RepairPlan> {
    let Some(ftyp) = read_header(src, 0, len)? else {
        return Err(invalid_data("too short to be an MP4"));
    };
    if &ftyp.kind != b"ftyp" || ftyp.size < 8 {
        return Err(invalid_data("does not start with ftyp"));
    }
    let moov_at = ftyp.size;
    let moov = read_header(src, moov_at, len)?
        .filter(|h| &h.kind == b"moov" && h.size >= h.header_len && moov_at + h.size <= len)
        .ok_or_else(|| invalid_data("no complete moov after ftyp: nothing is recoverable"))?;
    let moov_body = read_body(src, moov_at, &moov)?;
    let tracks = moov_tracks(&moov_body)?;
    let mehd_value_at = child(&moov_body, b"mvex")?.and_then(|(mvex_at, mvex)| {
        let (mehd_at, mehd) = child(mvex, b"mehd").ok()??;
        // Only a version 1 mehd (a 64-bit duration) is patched.
        (mehd.first() == Some(&1) && mehd.len() >= 12)
            .then_some(moov_at + moov.header_len + (mvex_at + mehd_at) as u64 + 4)
    });

    let mut ends: Vec<u64> = vec![0; tracks.len()];
    let mut index: Vec<(u32, Vec<RandomAccess>)> =
        tracks.iter().map(|&(id, _)| (id, Vec::new())).collect();
    let mut at = moov_at + moov.size;
    let mut fragments = 0;
    let mut already_complete = false;

    while let Some(head) = read_header(src, at, len)? {
        // A zero size is what a zero-filled range after a power loss reads
        // as, and a torn box runs past the end: either way, the good prefix
        // ends here.
        if head.size == 0 || at + head.size > len {
            break;
        }
        if head.size < head.header_len {
            return Err(invalid_data("a box smaller than its header"));
        }
        match &head.kind {
            b"moof" => {
                let moof = read_body(src, at, &head)?;
                let mdat_at = at + head.size;
                let Some(mdat) = read_header(src, mdat_at, len)? else {
                    break;
                };
                if mdat.size == 0 || mdat_at + mdat.size > len {
                    break;
                }
                if &mdat.kind != b"mdat" {
                    return Err(invalid_data("a moof not followed by its mdat"));
                }
                let payload = (mdat_at - at + mdat.header_len)..(mdat_at - at + mdat.size);
                let mut trafs = Vec::new();
                for (kind, _, traf) in children(&moof)? {
                    if &kind == b"traf" {
                        trafs.push(parse_traf(traf)?);
                    }
                }
                for (number, traf) in trafs.iter().enumerate() {
                    if traf.data.start < payload.start || traf.data.end > payload.end {
                        return Err(invalid_data("a trun's samples lie outside its mdat"));
                    }
                    let slot = tracks
                        .iter()
                        .position(|&(id, _)| id == traf.track_id)
                        .ok_or_else(|| invalid_data("a traf for a track moov does not declare"))?;
                    ends[slot] = ends[slot].max(traf.end_dts);
                    if let Some(time) = traf.sync_time {
                        index[slot].1.push(RandomAccess {
                            time,
                            moof_offset: at,
                            traf_number: u8::try_from(number + 1)
                                .map_err(|_| invalid_data("too many trafs"))?,
                        });
                    }
                }
                fragments += 1;
                at = mdat_at + mdat.size;
            }
            b"mfra" if at + head.size == len => {
                already_complete = true;
                break;
            }
            b"mfra" => return Err(invalid_data("data after a complete mfra")),
            other => {
                return Err(invalid_data(format!(
                    "unexpected {} box at {at}: not a file this writer produced",
                    String::from_utf8_lossy(other)
                )));
            }
        }
    }

    Ok(RepairPlan {
        keep: if already_complete { len } else { at },
        fragments,
        mfra: mfra(&index),
        mehd_value_at,
        duration: movie_duration(
            ends.iter()
                .zip(&tracks)
                .map(|(&end, &(_, scale))| (end, scale)),
        ),
        already_complete,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::path::PathBuf;
    use std::process::Command;

    // -- Pure helpers -------------------------------------------------------

    #[test]
    fn annex_b_splits_on_three_and_four_byte_start_codes() {
        let stream = [
            0, 0, 0, 1, 0x67, 0xAA, // SPS, four-byte start code
            0, 0, 1, 0x68, 0xBB, 0, 0, // PPS, then trailing_zero_8bits
            0, 0, 0, 1, 0x65, 0x01, 0x02, 0x03, // IDR slice
        ];
        let nals = annex_b_nals(&stream);
        assert_eq!(
            nals,
            vec![&[0x67, 0xAA][..], &[0x68, 0xBB][..], &[0x65, 1, 2, 3][..]]
        );
    }

    #[test]
    fn annex_b_keeps_emulation_prevention_bytes_inside_a_nal() {
        // `00 00 03 01` is escaped payload, not a start code.
        let stream = [0, 0, 1, 0x41, 0, 0, 3, 1, 0x80];
        assert_eq!(annex_b_nals(&stream), vec![&[0x41, 0, 0, 3, 1, 0x80][..]]);
    }

    #[test]
    fn avcc_prefixes_each_nal_with_its_length() {
        let stream = [0, 0, 0, 1, 0x09, 0xF0, 0, 0, 1, 0x65, 1, 2, 3];
        assert_eq!(
            annex_b_to_avcc(&stream),
            vec![0, 0, 0, 2, 0x09, 0xF0, 0, 0, 0, 4, 0x65, 1, 2, 3]
        );
        assert!(
            annex_b_to_avcc(&[1, 2, 3]).is_empty(),
            "no start code, no NALs"
        );
    }

    #[test]
    fn samples_drop_aud_and_repeated_parameter_sets() {
        let (sps, pps) = (&[0x67, 1][..], &[0x68, 2][..]);
        let au = [
            0, 0, 0, 1, 0x09, 0xF0, 0, 0, 0, 1, 0x67, 1, 0, 0, 0, 1, 0x68, 2, 0, 0, 1, 0x65, 7,
        ];
        let mut out = Vec::new();
        push_avc_sample(&mut out, &au, sps, pps).unwrap();
        assert_eq!(out, vec![0, 0, 0, 2, 0x65, 7]);

        let changed = [0, 0, 1, 0x67, 9, 0, 0, 1, 0x65, 7];
        assert!(push_avc_sample(&mut Vec::new(), &changed, sps, pps).is_err());
        assert!(
            push_avc_sample(&mut Vec::new(), &[0x65, 7], sps, pps).is_err(),
            "AVCC input"
        );
    }

    /// libx264's SPS for 1920x1080 High@4.2: 1088 lines of macroblocks,
    /// cropped by 8.
    const SPS_1080_HIGH: [u8; 27] = [
        0x67, 0x64, 0x00, 0x2a, 0xac, 0xd9, 0x40, 0x78, 0x02, 0x27, 0xe5, 0xc0, 0x44, 0x00, 0x00,
        0x03, 0x00, 0x04, 0x00, 0x00, 0x03, 0x01, 0xe0, 0x3c, 0x60, 0xc6, 0x58,
    ];
    /// libx264's SPS for 320x240 Constrained Baseline@2.1, uncropped.
    const SPS_240_BASELINE: [u8; 23] = [
        0x67, 0x42, 0xc0, 0x15, 0xd9, 0x01, 0x41, 0xfb, 0x01, 0x10, 0x00, 0x00, 0x03, 0x00, 0x10,
        0x00, 0x00, 0x07, 0x80, 0xf1, 0x62, 0xe4, 0x80,
    ];

    #[test]
    fn sps_high_profile_is_cropped_to_1080() {
        let info = parse_sps(&SPS_1080_HIGH).unwrap();
        assert_eq!(
            info,
            SpsInfo {
                profile_idc: 100,
                constraint_flags: 0,
                level_idc: 42,
                chroma_format_idc: 1,
                bit_depth_luma: 8,
                bit_depth_chroma: 8,
                width: 1920,
                height: 1080,
            }
        );
    }

    #[test]
    fn sps_baseline_has_no_chroma_fields() {
        let info = parse_sps(&SPS_240_BASELINE).unwrap();
        assert_eq!(
            (info.profile_idc, info.constraint_flags, info.level_idc),
            (66, 0xC0, 21)
        );
        assert_eq!((info.width, info.height), (320, 240));
    }

    #[test]
    fn sps_rejects_a_pps_and_a_truncated_sps() {
        assert!(parse_sps(&[0x68, 0xEE, 0x3C, 0x80]).is_err());
        assert!(parse_sps(&SPS_1080_HIGH[..8]).is_err());
    }

    #[test]
    fn rbsp_removes_emulation_prevention() {
        assert_eq!(
            rbsp(&[1, 0, 0, 3, 0, 0, 0, 3, 1]),
            vec![1, 0, 0, 0, 0, 0, 1]
        );
    }

    #[test]
    fn exp_golomb_reads_known_codes() {
        // 1 | 010 | 011 | 00100 | 00101 -> ue 0, 1, 2, 3; se(00101) = -2
        let data = [0b1010_0110, 0b0100_0010, 0b1000_0000];
        let mut r = Bits {
            data: &data,
            pos: 0,
        };
        assert_eq!(
            [
                r.ue().unwrap(),
                r.ue().unwrap(),
                r.ue().unwrap(),
                r.ue().unwrap()
            ],
            [0, 1, 2, 3]
        );
        assert_eq!(r.se().unwrap(), -2);
    }

    #[test]
    fn avcc_config_carries_the_high_profile_chroma_fields() {
        let info = parse_sps(&SPS_1080_HIGH).unwrap();
        let pps = [0x68, 0xeb, 0xe3, 0xcb, 0x22, 0xc0];
        let mut out = Vec::new();
        avc_config(&mut out, &SPS_1080_HIGH, &pps, &info);
        assert_eq!(&out[..6], &[1, 100, 0, 42, 0xFF, 0xE1]);
        assert_eq!(
            u16::from_be_bytes([out[6], out[7]]) as usize,
            SPS_1080_HIGH.len()
        );
        assert_eq!(out.len(), 6 + 2 + 27 + 1 + 2 + 6 + 4);
        assert_eq!(&out[out.len() - 4..], &[0xFD, 0xF8, 0xF8, 0]);

        let base = parse_sps(&SPS_240_BASELINE).unwrap();
        let mut out = Vec::new();
        avc_config(&mut out, &SPS_240_BASELINE, &pps, &base);
        assert_eq!(
            out.len(),
            6 + 2 + 23 + 1 + 2 + 6,
            "baseline has no trailing fields"
        );
    }

    #[test]
    fn audio_specific_config_known_values() {
        // AAC-LC, 48 kHz (index 3), stereo: 00010 0011 0010 000
        assert_eq!(audio_specific_config(48_000, 2).unwrap(), vec![0x11, 0x90]);
        // 44.1 kHz (index 4), mono: 00010 0100 0001 000
        assert_eq!(audio_specific_config(44_100, 1).unwrap(), vec![0x12, 0x08]);
        // 7.1 is channel configuration 7.
        assert_eq!(audio_specific_config(48_000, 8).unwrap(), vec![0x11, 0xB8]);
        // An unlisted rate escapes to 0xF and 24 explicit bits: 40 bits.
        let odd = audio_specific_config(50_000, 2).unwrap();
        assert_eq!(odd.len(), 5);
        assert_eq!(odd[0] >> 3, 2);
        assert_eq!(((odd[0] & 0x7) << 1) | (odd[1] >> 7), 0xF);
        assert!(audio_specific_config(48_000, 7).is_err());
        assert!(audio_specific_config(0, 2).is_err());
    }

    #[test]
    fn descriptor_lengths_use_seven_bit_groups() {
        let mut out = Vec::new();
        push_descriptor_len(&mut out, 5);
        push_descriptor_len(&mut out, 127);
        push_descriptor_len(&mut out, 128);
        push_descriptor_len(&mut out, 300);
        assert_eq!(out, vec![5, 127, 0x81, 0x00, 0x82, 0x2C]);
    }

    /// Read one descriptor back: its tag, and its body.
    fn read_descriptor(bytes: &[u8]) -> (u8, &[u8], usize) {
        let tag = bytes[0];
        let (mut len, mut at) = (0usize, 1);
        loop {
            let b = bytes[at];
            len = (len << 7) | usize::from(b & 0x7F);
            at += 1;
            if b & 0x80 == 0 {
                break;
            }
        }
        (tag, &bytes[at..at + len], at + len)
    }

    #[test]
    fn es_descriptor_lengths_nest_exactly() {
        let asc = audio_specific_config(48_000, 2).unwrap();
        let es = es_descriptor(2, &asc);
        let (tag, body, used) = read_descriptor(&es);
        assert_eq!(
            (tag, used),
            (0x03, es.len()),
            "ES_Descriptor spans the whole esds"
        );
        assert_eq!(&body[..3], &[0, 2, 0]);
        let (tag, config, used) = read_descriptor(&body[3..]);
        assert_eq!(tag, 0x04);
        assert_eq!(&config[..2], &[0x40, 0x15]);
        let (tag, specific, _) = read_descriptor(&config[13..]);
        assert_eq!((tag, specific), (0x05, &asc[..]));
        let (tag, sl, rest) = read_descriptor(&body[3 + used..]);
        assert_eq!((tag, sl), (0x06, &[0x02][..]));
        assert_eq!(3 + used + rest, body.len());
    }

    #[test]
    fn movie_duration_rounds_up_to_the_longest_track() {
        let d = movie_duration([(90_000 * 3 + 1, 90_000), (48_000 * 2, 48_000)].into_iter());
        assert_eq!(d, 3001);
        assert_eq!(movie_duration(std::iter::empty()), 0);
    }

    // -- A test-only box walker ---------------------------------------------

    /// A box as the test walker sees it: its kind, offset, size and children.
    #[derive(Debug)]
    struct Node {
        kind: String,
        at: usize,
        size: usize,
        body: std::ops::Range<usize>,
        children: Vec<Node>,
    }

    const CONTAINERS: [&str; 11] = [
        "moov", "trak", "mdia", "minf", "dinf", "stbl", "mvex", "moof", "traf", "mfra", "dref",
    ];

    /// Walk `bytes[range]`, asserting every box's size lies inside its parent
    /// and the children tile the parent exactly.
    fn walk(bytes: &[u8], range: std::ops::Range<usize>) -> Vec<Node> {
        let mut out = Vec::new();
        let mut at = range.start;
        while at < range.end {
            let size = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
            let kind = String::from_utf8_lossy(&bytes[at + 4..at + 8]).into_owned();
            assert!(
                size >= 8 && at + size <= range.end,
                "{kind} at {at} overruns its parent"
            );
            let mut body = at + 8..at + size;
            let children = match kind.as_str() {
                "dref" | "stsd" => {
                    body.start += 8; // version, flags, entry_count
                    walk(bytes, body.clone())
                }
                "avc1" => walk(bytes, at + 8 + 78..at + size),
                "mp4a" => walk(bytes, at + 8 + 28..at + size),
                k if CONTAINERS.contains(&k) => walk(bytes, body.clone()),
                _ => Vec::new(),
            };
            out.push(Node {
                kind,
                at,
                size,
                body,
                children,
            });
            at += size;
        }
        assert_eq!(at, range.end, "children must tile their parent exactly");
        out
    }

    fn find<'a>(nodes: &'a [Node], path: &str) -> Vec<&'a Node> {
        let mut current: Vec<&Node> = nodes.iter().collect();
        for (i, part) in path.split('/').enumerate() {
            if i > 0 {
                current = current.iter().flat_map(|n| n.children.iter()).collect();
            }
            current.retain(|n| n.kind == part);
        }
        current
    }

    fn kinds(nodes: &[Node]) -> Vec<&str> {
        nodes.iter().map(|n| n.kind.as_str()).collect()
    }

    fn be32(bytes: &[u8], at: usize) -> u32 {
        u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap())
    }

    fn be64(bytes: &[u8], at: usize) -> u64 {
        u64::from_be_bytes(bytes[at..at + 8].try_into().unwrap())
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nr-mp4-write-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A video track from the baseline SPS and three audio tracks, fed fake
    /// payloads. Box structure only: nothing decodes these.
    fn synthetic_file(path: &Path, fragments: usize, finish: bool) -> Vec<u8> {
        let pps = [0x68, 0xce, 0x38, 0x80];
        let tracks = vec![
            Track::h264(&SPS_240_BASELINE, &pps).unwrap(),
            Track::aac_lc(48_000, 2).unwrap(),
            Track::aac_lc(48_000, 1).unwrap(),
            Track::aac_lc(48_000, 2).unwrap(),
        ];
        let mut w = Writer::create(path, tracks).unwrap();
        let mut audio_pts = 0u64;
        for f in 0..fragments {
            if f > 0 {
                w.flush_fragment().unwrap();
            }
            for frame in 0..10u64 {
                let pts = (f as u64 * 10 + frame) * 1500;
                let key = frame == 0;
                let au = [
                    0,
                    0,
                    0,
                    1,
                    if key { 0x65 } else { 0x41 },
                    0x88,
                    frame as u8,
                    1,
                ];
                w.write_sample(0, pts, 1500, key, &au).unwrap();
                // Audio up to this video frame's end.
                while audio_pts * 90_000 < (pts + 1500) * 48_000 {
                    for track in 1..4 {
                        w.write_sample(track, audio_pts, 1024, false, &[0x21, track as u8, 3])
                            .unwrap();
                    }
                    audio_pts += 1024;
                }
            }
        }
        if finish {
            w.finish().unwrap();
        } else {
            w.flush_fragment().unwrap();
            drop(w);
        }
        std::fs::read(path).unwrap()
    }

    #[test]
    fn box_sizes_tile_and_the_layout_is_the_documented_one() {
        let dir = temp_dir("layout");
        let bytes = synthetic_file(&dir.join("a.mp4"), 3, true);
        let top = walk(&bytes, 0..bytes.len());
        assert_eq!(
            kinds(&top),
            [
                "ftyp", "moov", "moof", "mdat", "moof", "mdat", "moof", "mdat", "mfra"
            ]
        );

        let moov = &find(&top, "moov")[0];
        assert_eq!(
            kinds(&moov.children),
            ["mvhd", "trak", "trak", "trak", "trak", "mvex"]
        );
        let trak = &moov.children[1];
        assert_eq!(kinds(&trak.children), ["tkhd", "mdia"]);
        assert_eq!(
            kinds(&find(&top, "moov/trak/mdia/minf/stbl")[0].children),
            ["stsd", "stts", "stsc", "stsz", "stco"]
        );
        assert_eq!(find(&top, "moov/trak/mdia/minf/vmhd").len(), 1);
        assert_eq!(find(&top, "moov/trak/mdia/minf/smhd").len(), 3);
        assert_eq!(
            find(&top, "moov/trak/mdia/minf/stbl/stsd/avc1/avcC").len(),
            1
        );
        assert_eq!(
            find(&top, "moov/trak/mdia/minf/stbl/stsd/mp4a/esds").len(),
            3
        );
        assert_eq!(
            kinds(&find(&top, "moov/mvex")[0].children),
            ["mehd", "trex", "trex", "trex", "trex"]
        );

        // tkhd: video and the first audio track enabled, the stems not.
        let tkhd_flags: Vec<u32> = find(&top, "moov/trak/tkhd")
            .iter()
            .map(|n| be32(&bytes, n.body.start) & 0xFF_FFFF)
            .collect();
        assert_eq!(tkhd_flags, [3, 3, 2, 2]);

        // mehd holds the longest track's duration: the video is 30 frames
        // at 60 fps (500 ms), and the audio runs to 24 frames of 1024
        // (512 ms).
        let mehd = find(&top, "moov/mvex/mehd")[0];
        assert_eq!(be64(&bytes, mehd.body.start + 4), 512);

        // Each moof has mfhd and one traf per track, numbered from 1.
        for (i, moof) in find(&top, "moof").iter().enumerate() {
            assert_eq!(
                kinds(&moof.children),
                ["mfhd", "traf", "traf", "traf", "traf"]
            );
            assert_eq!(be32(&bytes, moof.children[0].body.start + 4), i as u32 + 1);
            for traf in &moof.children[1..] {
                assert_eq!(kinds(&traf.children), ["tfhd", "tfdt", "trun"]);
                assert_eq!(bytes[traf.children[1].body.start], 1, "tfdt is version 1");
            }
        }

        // mfra: four tfras and an mfro whose size is the mfra's.
        let mfra = find(&top, "mfra")[0];
        assert_eq!(
            kinds(&mfra.children),
            ["tfra", "tfra", "tfra", "tfra", "mfro"]
        );
        assert_eq!(
            be32(&bytes, mfra.children[4].body.start + 4) as usize,
            mfra.size
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn trun_offsets_point_at_each_tracks_samples_in_the_mdat() {
        let dir = temp_dir("offsets");
        let bytes = synthetic_file(&dir.join("a.mp4"), 2, true);
        let top = walk(&bytes, 0..bytes.len());
        let moofs = find(&top, "moof");
        let mdats = find(&top, "mdat");
        for (moof, mdat) in moofs.iter().zip(&mdats) {
            let mut expected = mdat.body.start;
            for traf in &moof.children[1..] {
                let trun = &traf.children[2];
                let count = be32(&bytes, trun.body.start + 4) as usize;
                let offset = be32(&bytes, trun.body.start + 8) as usize;
                assert_eq!(
                    moof.at + offset,
                    expected,
                    "samples are contiguous, in traf order"
                );
                let track_id = be32(&bytes, traf.children[0].body.start + 4);
                let first = moof.at + offset;
                if track_id == 1 {
                    // AVCC: a 4-byte length, then the slice NAL.
                    assert_eq!(be32(&bytes, first), 4);
                    assert_eq!(
                        bytes[first + 4],
                        0x65,
                        "each fragment opens on the keyframe"
                    );
                } else {
                    assert_eq!(&bytes[first..first + 2], &[0x21, track_id as u8 - 1]);
                }
                let per_sample = 16
                    - if be32(&bytes, trun.body.start) & TRUN_CTO == 0 {
                        4
                    } else {
                        0
                    };
                let sizes: usize = (0..count)
                    .map(|i| be32(&bytes, trun.body.start + 12 + i * per_sample + 4) as usize)
                    .sum();
                expected += sizes;
            }
            assert_eq!(
                expected,
                mdat.at + mdat.size,
                "the trafs account for the whole mdat"
            );
        }

        // Each tfra entry names a moof and the right traf.
        let mfra = find(&top, "mfra")[0];
        for tfra in &mfra.children[..4] {
            let count = be32(&bytes, tfra.body.start + 12) as usize;
            assert_eq!(count, 2);
            for i in 0..count {
                let entry = tfra.body.start + 16 + i * 19;
                assert_eq!(be64(&bytes, entry + 8) as usize, moofs[i].at);
            }
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn write_sample_refuses_what_it_cannot_describe() {
        let dir = temp_dir("refuse");
        let pps = [0x68, 0xce, 0x38, 0x80];
        let tracks = vec![
            Track::h264(&SPS_240_BASELINE, &pps).unwrap(),
            Track::aac_lc(48_000, 2).unwrap(),
        ];
        let mut w = Writer::create(&dir.join("a.mp4"), tracks).unwrap();
        assert!(
            w.write_sample(2, 0, 1024, true, &[1]).is_err(),
            "no such track"
        );
        assert!(
            w.write_sample(0, 0, 1500, false, &[0, 0, 1, 0x41, 1])
                .is_err(),
            "starts on a P-frame"
        );
        assert!(
            w.write_sample(0, 0, 0, true, &[0, 0, 1, 0x65, 1]).is_err(),
            "zero duration"
        );
        assert!(
            w.write_sample(1, 0, 1024, true, &[]).is_err(),
            "empty sample"
        );
        w.write_sample(0, 0, 1500, true, &[0, 0, 1, 0x65, 1])
            .unwrap();
        let far = u64::from(u32::MAX) * 2;
        assert!(
            w.write_sample(0, far, 1500, false, &[0, 0, 1, 0x41, 1])
                .is_err(),
            "offset overflow"
        );
        assert!(Writer::create(&dir.join("b.mp4"), Vec::new()).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn negative_composition_offsets_use_trun_version_1() {
        let dir = temp_dir("bframes");
        let path = dir.join("a.mp4");
        let pps = [0x68, 0xce, 0x38, 0x80];
        let mut w =
            Writer::create(&path, vec![Track::h264(&SPS_240_BASELINE, &pps).unwrap()]).unwrap();
        // I P B B in decode order; display order I B B P.
        for (pts, key, nal) in [
            (0, true, 0x65),
            (4500, false, 0x41),
            (1500, false, 0x01),
            (3000, false, 0x01),
        ] {
            w.write_sample(0, pts, 1500, key, &[0, 0, 1, nal, 0x88])
                .unwrap();
        }
        w.finish().unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let top = walk(&bytes, 0..bytes.len());
        let trun = find(&top, "moof/traf/trun")[0];
        assert_eq!(bytes[trun.body.start], 1, "version 1: signed offsets");
        let offsets: Vec<i32> = (0..4)
            .map(|i| be32(&bytes, trun.body.start + 12 + i * 16 + 12) as i32)
            .collect();
        // dts = 0, 1500, 3000, 4500.
        assert_eq!(offsets, [0, 3000, -1500, -1500]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn repair_plan_cuts_at_the_last_complete_fragment_for_every_offset() {
        let dir = temp_dir("plan");
        let bytes = synthetic_file(&dir.join("a.mp4"), 3, true);
        let top = walk(&bytes, 0..bytes.len());
        let moofs = find(&top, "moof");
        let mfra = find(&top, "mfra")[0];
        let last_moof = moofs[2].at;
        let fragments_end = mfra.at;

        let plan_at = |k: usize| plan_repair(&mut io::Cursor::new(&bytes[..k]), k as u64).unwrap();
        for k in last_moof..fragments_end {
            let plan = plan_at(k);
            assert_eq!(
                (plan.keep, plan.fragments),
                (last_moof as u64, 2),
                "cut at {k}"
            );
            assert!(!plan.already_complete);
        }
        // Killed between fragments, or while writing the mfra.
        for k in fragments_end..bytes.len() {
            let plan = plan_at(k);
            assert_eq!(
                (plan.keep, plan.fragments),
                (fragments_end as u64, 3),
                "cut at {k}"
            );
        }
        // A finished file is left alone.
        let plan = plan_at(bytes.len());
        assert!(plan.already_complete);
        assert_eq!(plan.keep, bytes.len() as u64);
        // Killed inside moov: nothing to recover.
        assert!(
            plan_repair(
                &mut io::Cursor::new(&bytes[..moofs[0].at - 1]),
                moofs[0].at as u64 - 1
            )
            .is_err()
        );
        // With the moov and no fragments, the result is an empty file.
        assert_eq!(plan_at(moofs[0].at).fragments, 0);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn repair_rebuilds_the_same_mfra_and_mehd_finish_wrote() {
        let dir = temp_dir("same");
        let finished = synthetic_file(&dir.join("a.mp4"), 3, true);
        let killed_path = dir.join("b.mp4");
        let killed = synthetic_file(&killed_path, 3, false);
        // Plus a torn fourth fragment's first bytes.
        let mut torn = killed.clone();
        torn.extend_from_slice(&finished[killed.len() - 100..killed.len() - 40]);
        std::fs::write(&killed_path, &torn).unwrap();

        let report = repair(&killed_path).unwrap();
        assert_eq!(report.fragments, 3);
        assert_eq!(report.kept_bytes, killed.len() as u64);
        assert_eq!(report.removed_bytes, 60);
        assert_eq!(
            std::fs::read(&killed_path).unwrap(),
            finished,
            "byte-identical to finish()"
        );

        let again = repair(&killed_path).unwrap();
        assert!(again.already_complete);
        assert_eq!(std::fs::read(&killed_path).unwrap(), finished);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn repair_refuses_a_file_it_did_not_write() {
        let dir = temp_dir("foreign");
        let path = dir.join("a.mp4");
        let mut bytes = synthetic_file(&path, 1, false);
        // A complete box this writer never produces.
        bytes.extend_from_slice(&[0, 0, 0, 12, b'f', b'r', b'e', b'e', 1, 2, 3, 4]);
        std::fs::write(&path, &bytes).unwrap();
        assert!(repair(&path).is_err());
        assert_eq!(
            std::fs::read(&path).unwrap(),
            bytes,
            "refused, and untouched"
        );
        std::fs::write(&path, b"not an mp4 at all").unwrap();
        assert!(repair(&path).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    // -- Against ffmpeg -----------------------------------------------------

    /// ffmpeg and ffprobe from `PATH`, or `None` so the test can skip: CI's
    /// Linux runner has neither.
    /// What this module writes, `read` and startup recovery must understand:
    /// a finished file and a killed one both summarise as fragmented, with
    /// every track and fragment counted, and recovery would remux the killed
    /// one with all three audio tracks.
    #[test]
    fn the_reader_and_recovery_understand_what_the_writer_wrote() {
        use crate::db::reconcile::{RecoveryAction, recovery_action};
        let dir = temp_dir("read-back");
        for finish in [true, false] {
            let path = dir.join(format!("f-{finish}.mp4"));
            synthetic_file(&path, 3, finish);
            let summary = super::super::summarize(&mut std::fs::File::open(&path).unwrap()).unwrap();
            assert!(summary.ftyp && summary.moov_complete && summary.mvex, "{summary:?}");
            assert_eq!((summary.tracks, summary.audio_tracks), (4, 3));
            assert_eq!(summary.complete_fragments, 3);
            assert_eq!(summary.mfra, finish);
            assert!(summary.truncated.is_none(), "{summary:?}");
            assert!(
                matches!(recovery_action(&summary), RecoveryAction::Remux { audio_tracks: 3, truncate_to: None }),
                "{:?}",
                recovery_action(&summary)
            );
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    fn tools() -> Option<(PathBuf, PathBuf)> {
        let find = |name: &str| {
            let exe = if cfg!(windows) {
                format!("{name}.exe")
            } else {
                name.to_string()
            };
            std::env::split_paths(&std::env::var_os("PATH")?)
                .map(|dir| dir.join(&exe))
                .find(|p| p.is_file())
        };
        let found = find("ffmpeg").zip(find("ffprobe"));
        if found.is_none() {
            eprintln!("skipping: ffmpeg and ffprobe are not both on PATH");
        }
        found
    }

    fn run(cmd: &mut Command) -> std::process::Output {
        let out = cmd.output().expect("spawn");
        assert!(
            out.status.success(),
            "{cmd:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }

    /// One H.264 access unit, in Annex B, with its timing in 90 kHz ticks.
    struct Frame {
        bytes: Vec<u8>,
        pts: u64,
        duration: u32,
        keyframe: bool,
    }

    /// Group an Annex B stream's NALs into access units: a new one starts at
    /// an AUD, SPS, PPS or SEI, or at a slice with `first_mb_in_slice = 0`,
    /// once the current one holds a slice.
    fn access_units(stream: &[u8]) -> Vec<Vec<u8>> {
        let mut units: Vec<Vec<u8>> = Vec::new();
        let mut current = Vec::new();
        let mut has_slice = false;
        for nal in annex_b_nals(stream) {
            let kind = nal_type(nal);
            let slice = kind == 1 || kind == NAL_IDR;
            let starts = matches!(kind, 6..=9) || (slice && nal[1] & 0x80 != 0);
            if starts && has_slice {
                units.push(std::mem::take(&mut current));
                has_slice = false;
            }
            current.extend_from_slice(&[0, 0, 0, 1]);
            current.extend_from_slice(nal);
            has_slice |= slice;
        }
        if has_slice {
            units.push(current);
        }
        units
    }

    fn is_keyframe(au: &[u8]) -> bool {
        annex_b_nals(au).iter().any(|n| nal_type(n) == NAL_IDR)
    }

    /// x264 at 320x240, 60 fps, a 2 s GOP. Without B-frames the stream is
    /// written straight to Annex B and timed by frame index; with them, the
    /// timing has to come from ffmpeg's own mux, so it is encoded to MP4 at
    /// 90 kHz, probed for pts, and converted back to Annex B.
    fn h264_fixture(
        ffmpeg: &Path,
        ffprobe: &Path,
        dir: &Path,
        seconds: u32,
        b_frames: bool,
    ) -> Vec<Frame> {
        let es = dir.join("video.h264");
        let source = format!("testsrc2=size=320x240:rate=60:duration={seconds}");
        let encode = |out: &Path, extra: &[&str]| {
            run(Command::new(ffmpeg)
                .args(["-v", "error", "-y", "-f", "lavfi", "-i", &source])
                .args(["-c:v", "libx264", "-g", "120", "-pix_fmt", "yuv420p"])
                .args(["-bf", if b_frames { "2" } else { "0" }])
                .args(extra)
                .arg(out));
        };
        if !b_frames {
            encode(&es, &["-f", "h264"]);
            let units = access_units(&std::fs::read(&es).unwrap());
            return units
                .into_iter()
                .enumerate()
                .map(|(i, bytes)| Frame {
                    keyframe: is_keyframe(&bytes),
                    bytes,
                    pts: i as u64 * 1500,
                    duration: 1500,
                })
                .collect();
        }
        let mp4 = dir.join("video-b.mp4");
        encode(&mp4, &["-video_track_timescale", "90000"]);
        run(Command::new(ffmpeg)
            .args(["-v", "error", "-y", "-i"])
            .arg(&mp4)
            .args(["-c:v", "copy", "-bsf:v", "h264_mp4toannexb", "-f", "h264"])
            .arg(&es));
        let probe = run(Command::new(ffprobe)
            .args([
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_entries",
                "packet=pts,dts,duration",
            ])
            .args(["-of", "json"])
            .arg(&mp4));
        let json: Value = serde_json::from_slice(&probe.stdout).unwrap();
        let packets = json["packets"].as_array().unwrap();
        let units = access_units(&std::fs::read(&es).unwrap());
        assert_eq!(units.len(), packets.len(), "one access unit per packet");
        let pts: Vec<i64> = packets.iter().map(|p| p["pts"].as_i64().unwrap()).collect();
        let base = *pts.iter().min().unwrap();
        units
            .into_iter()
            .zip(pts)
            .map(|(bytes, pts)| Frame {
                keyframe: is_keyframe(&bytes),
                bytes,
                pts: (pts - base) as u64,
                duration: 1500,
            })
            .collect()
    }

    /// Raw AAC-LC access units from an ADTS stream, with the rate and
    /// channel count its headers declare.
    fn aac_fixture(
        ffmpeg: &Path,
        dir: &Path,
        name: &str,
        hz: u32,
        channels: u16,
        seconds: u32,
    ) -> (Vec<Vec<u8>>, u32, u16) {
        let path = dir.join(format!("{name}.aac"));
        run(Command::new(ffmpeg)
            .args(["-v", "error", "-y", "-f", "lavfi"])
            .args([
                "-i",
                &format!("sine=frequency={hz}:sample_rate=48000:duration={seconds}"),
            ])
            .args([
                "-ac",
                &channels.to_string(),
                "-c:a",
                "aac",
                "-b:a",
                "128k",
                "-f",
                "adts",
            ])
            .arg(&path));
        let adts = std::fs::read(&path).unwrap();
        let (mut frames, mut at) = (Vec::new(), 0);
        let (mut rate, mut chans) = (0, 0);
        while at + 7 <= adts.len() {
            let h = &adts[at..];
            assert_eq!((h[0], h[1] & 0xF0), (0xFF, 0xF0), "ADTS sync at {at}");
            let protection_absent = h[1] & 1 == 1;
            rate = AAC_SAMPLE_RATES[usize::from((h[2] >> 2) & 0xF)];
            chans = u16::from(((h[2] & 1) << 2) | (h[3] >> 6));
            let len =
                (usize::from(h[3] & 3) << 11) | (usize::from(h[4]) << 3) | usize::from(h[5] >> 5);
            assert_eq!(h[6] & 3, 0, "one raw data block per frame");
            let header = if protection_absent { 7 } else { 9 };
            frames.push(adts[at + header..at + len].to_vec());
            at += len;
        }
        (frames, rate, chans)
    }

    struct Written {
        video_frames: usize,
        audio_frames: Vec<usize>,
        seconds: f64,
    }

    /// Write video plus `audio` tracks the way the own backend will: in time
    /// order, flushing immediately before each keyframe after the first.
    /// Without `finish`, the last fragment is flushed and the writer dropped,
    /// which is what a killed process leaves.
    fn write_file(
        tools: &(PathBuf, PathBuf),
        dir: &Path,
        out: &Path,
        audio: usize,
        b_frames: bool,
        finish: bool,
    ) -> Written {
        let seconds = 5;
        let video = h264_fixture(&tools.0, &tools.1, dir, seconds, b_frames);
        let channel_counts = [2u16, 1, 2, 1];
        let audio_streams: Vec<(Vec<Vec<u8>>, u32, u16)> = (0..audio)
            .map(|i| {
                aac_fixture(
                    &tools.0,
                    dir,
                    &format!("a{i}"),
                    220 * (i as u32 + 2),
                    channel_counts[i],
                    seconds,
                )
            })
            .collect();

        let mut tracks = vec![Track::h264_from_annex_b(&video[0].bytes).unwrap()];
        for (_, rate, channels) in &audio_streams {
            tracks.push(Track::aac_lc(*rate, *channels).unwrap());
        }
        let mut w = Writer::create(out, tracks).unwrap();

        // (seconds, track, index), merged into one decode-time order.
        let mut events: Vec<(f64, usize, usize)> = Vec::new();
        let mut dts = 0u64;
        for (i, f) in video.iter().enumerate() {
            events.push((dts as f64 / 90_000.0, 0, i));
            dts += u64::from(f.duration);
        }
        for (t, (frames, rate, _)) in audio_streams.iter().enumerate() {
            for i in 0..frames.len() {
                events.push(((i * 1024) as f64 / f64::from(*rate), t + 1, i));
            }
        }
        events.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));

        for &(_, track, i) in &events {
            if track == 0 {
                let f = &video[i];
                if f.keyframe && i > 0 {
                    w.flush_fragment().unwrap();
                }
                w.write_sample(0, f.pts, f.duration, f.keyframe, &f.bytes)
                    .unwrap();
            } else {
                let frame = &audio_streams[track - 1].0[i];
                w.write_sample(track, i as u64 * 1024, 1024, true, frame)
                    .unwrap();
            }
        }
        if finish {
            w.finish().unwrap();
        } else {
            w.flush_fragment().unwrap();
        }
        Written {
            video_frames: video.len(),
            audio_frames: audio_streams.iter().map(|a| a.0.len()).collect(),
            seconds: f64::from(seconds),
        }
    }

    fn ffprobe_json(ffprobe: &Path, file: &Path) -> Value {
        let out = run(Command::new(ffprobe)
            .args([
                "-v",
                "error",
                "-count_packets",
                "-show_streams",
                "-show_format",
                "-of",
                "json",
            ])
            .arg(file));
        serde_json::from_slice(&out.stdout).unwrap()
    }

    /// A full decode of every stream. It must succeed and say nothing.
    fn assert_decodes_cleanly(ffmpeg: &Path, file: &Path) {
        let out = Command::new(ffmpeg)
            .args(["-v", "error", "-i"])
            .arg(file)
            .args(["-map", "0", "-f", "null", "-"])
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            out.status.success() && stderr.trim().is_empty(),
            "{} decode: {stderr}",
            file.display()
        );
    }

    fn as_f64(v: &Value) -> f64 {
        v.as_str()
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| panic!("not a number: {v}"))
    }

    fn check_file(tools: &(PathBuf, PathBuf), file: &Path, written: &Written) {
        let json = ffprobe_json(&tools.1, file);
        let streams = json["streams"].as_array().unwrap();
        assert_eq!(streams.len(), 1 + written.audio_frames.len(), "{json:#}");

        let v = &streams[0];
        assert_eq!(v["codec_name"], "h264");
        assert_eq!(
            (v["width"].as_u64(), v["height"].as_u64()),
            (Some(320), Some(240))
        );
        assert_eq!(as_f64(&v["nb_read_packets"]) as usize, written.video_frames);
        assert_eq!(v["disposition"]["default"], 1);
        let duration = as_f64(&v["duration"]);
        assert!(
            (duration - written.seconds).abs() < 0.05,
            "video duration {duration}"
        );

        for (i, a) in streams[1..].iter().enumerate() {
            assert_eq!(a["codec_name"], "aac", "{a:#}");
            assert_eq!(a["profile"], "LC");
            assert_eq!(a["sample_rate"], "48000");
            assert_eq!(a["channels"].as_u64(), Some(u64::from([2u16, 1, 2, 1][i])));
            assert_eq!(
                as_f64(&a["nb_read_packets"]) as usize,
                written.audio_frames[i]
            );
            assert_eq!(
                a["disposition"]["default"],
                u64::from(i == 0),
                "only track 0 is default"
            );
            let duration = as_f64(&a["duration"]);
            assert!(
                (duration - written.seconds).abs() < 0.1,
                "audio {i} duration {duration}"
            );
        }
        let format_duration = as_f64(&json["format"]["duration"]);
        assert!(
            (format_duration - written.seconds).abs() < 0.1,
            "format duration {format_duration}"
        );
        assert_decodes_cleanly(&tools.0, file);
    }

    fn written_file_is_valid(audio: usize, b_frames: bool) {
        let Some(tools) = tools() else { return };
        let dir = temp_dir(&format!("ffmpeg-{audio}-{b_frames}"));
        let out = dir.join("out.mp4");
        let written = write_file(&tools, &dir, &out, audio, b_frames, true);
        check_file(&tools, &out, &written);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn video_and_one_audio_track_probe_and_decode_cleanly() {
        written_file_is_valid(1, false);
    }

    #[test]
    fn video_and_two_audio_tracks_probe_and_decode_cleanly() {
        written_file_is_valid(2, false);
    }

    #[test]
    fn video_and_four_audio_tracks_probe_and_decode_cleanly() {
        written_file_is_valid(4, false);
    }

    #[test]
    fn b_frames_probe_and_decode_cleanly() {
        written_file_is_valid(1, true);
    }

    #[test]
    fn b_frame_presentation_order_survives_the_round_trip() {
        let Some(tools) = tools() else { return };
        let dir = temp_dir("bframe-pts");
        let out = dir.join("out.mp4");
        write_file(&tools, &dir, &out, 0, true, true);
        let probe = run(Command::new(&tools.1)
            .args([
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_entries",
                "packet=pts,dts",
            ])
            .args(["-of", "json"])
            .arg(&out));
        let json: Value = serde_json::from_slice(&probe.stdout).unwrap();
        let mut pts: Vec<i64> = json["packets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["pts"].as_i64().unwrap())
            .collect();
        let reordered = pts.windows(2).any(|w| w[1] < w[0]);
        assert!(reordered, "the fixture really has B-frames");
        pts.sort_unstable();
        let base = pts[0];
        for (i, p) in pts.iter().enumerate() {
            assert_eq!(
                p - base,
                i as i64 * 1500,
                "every display slot filled exactly once"
            );
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_unfinished_file_already_plays() {
        let Some(tools) = tools() else { return };
        let dir = temp_dir("unfinished");
        let out = dir.join("out.mp4");
        let written = write_file(&tools, &dir, &out, 2, false, false);
        // No mfra and a zero mehd: ffprobe still finds every packet.
        let json = ffprobe_json(&tools.1, &out);
        assert_eq!(
            as_f64(&json["streams"][0]["nb_read_packets"]) as usize,
            written.video_frames
        );
        assert_decodes_cleanly(&tools.0, &out);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn repair_of_a_killed_file_decodes_cleanly() {
        let Some(tools) = tools() else { return };
        let dir = temp_dir("repair");
        let out = dir.join("out.mp4");
        write_file(&tools, &dir, &out, 2, false, false);
        let bytes = std::fs::read(&out).unwrap();
        let top = walk(&bytes, 0..bytes.len());
        let moofs = find(&top, "moof");
        assert!(moofs.len() >= 3, "5 s at a 2 s GOP");
        let last = moofs[moofs.len() - 1];
        let last_mdat = find(&top, "mdat").last().unwrap().at;

        // Inside the moof's header, inside the moof, on the mdat header,
        // inside the mdat, and one byte short of the end.
        let cuts = [
            last.at + 3,
            last.at + last.size / 2,
            last_mdat + 4,
            last_mdat + 1000,
            bytes.len() - 1,
        ];
        for cut in cuts {
            let killed = dir.join(format!("killed-{cut}.mp4"));
            std::fs::write(&killed, &bytes[..cut]).unwrap();
            let report = repair(&killed).unwrap();
            assert_eq!(report.fragments, moofs.len() - 1, "cut at {cut}");
            assert_eq!(report.kept_bytes, last.at as u64);

            // Every sample of the earlier fragments survives.
            let repaired = std::fs::read(&killed).unwrap();
            walk(&repaired, 0..repaired.len());
            // Byte for byte, apart from mehd's duration being filled in.
            let moov = find(&top, "moov")[0];
            let fragments = moov.at + moov.size..last.at;
            assert_eq!(repaired[fragments.clone()], bytes[fragments]);
            let mehd = find(&walk(&repaired, 0..repaired.len()), "moov/mvex/mehd")[0]
                .body
                .start
                + 4;
            assert_eq!(be64(&bytes, mehd), 0, "a killed file's mehd is unset");
            // Two whole 2 s GOPs, and the audio frame that straddles the
            // second keyframe: 1024 samples at 48 kHz is 21.3 ms.
            let duration = be64(&repaired, mehd);
            assert!((4000..4022).contains(&duration), "mehd {duration} ms");
            let json = ffprobe_json(&tools.1, &killed);
            let packets = as_f64(&json["streams"][0]["nb_read_packets"]) as usize;
            assert_eq!(
                packets,
                120 * (moofs.len() - 1),
                "whole GOPs of the earlier fragments"
            );
            assert_decodes_cleanly(&tools.0, &killed);
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
