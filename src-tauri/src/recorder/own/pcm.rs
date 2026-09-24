//! Endpoint samples to what the mixer and the AAC encoder take, with no
//! Windows in it.
//!
//! Ported from `spikes/p0c-video/src/pcm.rs`. A shared-mode endpoint delivers
//! whatever its mix format is: usually 32-bit float, sometimes 16- or 24-bit
//! integer, with anywhere from one to eight channels. Microsoft's AAC encoder
//! takes 16-bit PCM, one, two or six channels, at 44.1 or 48 kHz.
//!
//! Two paths, both stereo, both keeping the first two channels (a mono source
//! is doubled). That is not a downmix, and a 5.1 source loses its centre:
//!
//! - [`to_stereo_i16`] is the spike's, for a source that goes straight to an
//!   encoder.
//! - [`to_stereo_f32`] keeps full precision for the mixer (#238), which sums
//!   sources into track 0 before [`f32_to_i16`] quantises the result once.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleFormat {
    F32,
    I16,
    /// 24-bit in 3 bytes.
    I24,
    /// 24- or 32-bit valid bits in a 4-byte container.
    I32,
}

impl SampleFormat {
    pub fn bytes(self) -> usize {
        match self {
            SampleFormat::F32 | SampleFormat::I32 => 4,
            SampleFormat::I16 => 2,
            SampleFormat::I24 => 3,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            SampleFormat::F32 => "32-bit float",
            SampleFormat::I16 => "16-bit PCM",
            SampleFormat::I24 => "24-bit PCM",
            SampleFormat::I32 => "32-bit PCM",
        }
    }
}

/// The rates Microsoft's AAC encoder accepts.
pub fn aac_rate_supported(rate: u32) -> bool {
    rate == 44_100 || rate == 48_000
}

fn sample(data: &[u8], format: SampleFormat) -> i16 {
    match format {
        SampleFormat::F32 => {
            let v = f32::from_le_bytes([data[0], data[1], data[2], data[3]]);
            (v.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16
        }
        SampleFormat::I16 => i16::from_le_bytes([data[0], data[1]]),
        SampleFormat::I24 => i16::from_le_bytes([data[1], data[2]]),
        SampleFormat::I32 => i16::from_le_bytes([data[2], data[3]]),
    }
}

/// One sample as f32, where an integer format's full scale is -1.0..1.0 (so
/// -32768 is exactly -1.0). A float source is passed through unclamped:
/// clamping is the mixer's job, after it has summed its sources.
fn sample_f32(data: &[u8], format: SampleFormat) -> f32 {
    match format {
        SampleFormat::F32 => f32::from_le_bytes([data[0], data[1], data[2], data[3]]),
        SampleFormat::I16 => f32::from(i16::from_le_bytes([data[0], data[1]])) / 32_768.0,
        SampleFormat::I24 => {
            // Sign-extend by putting the three bytes at the top of an i32.
            let v = i32::from_le_bytes([0, data[0], data[1], data[2]]) >> 8;
            v as f32 / 8_388_608.0
        }
        SampleFormat::I32 => {
            let v = i32::from_le_bytes([data[0], data[1], data[2], data[3]]);
            (f64::from(v) / 2_147_483_648.0) as f32
        }
    }
}

/// Walks `frames` interleaved frames and emits the first two channels of each,
/// doubling a mono source. Short input is treated as silence rather than read
/// past.
fn stereo<T: Copy + Default>(
    data: &[u8],
    format: SampleFormat,
    channels: u16,
    frames: u32,
    read: fn(&[u8], SampleFormat) -> T,
) -> Vec<T> {
    let channels = usize::from(channels.max(1));
    let width = format.bytes();
    let stride = width * channels;
    let mut out = Vec::with_capacity(frames as usize * 2);
    for frame in 0..frames as usize {
        let base = frame * stride;
        if base + stride > data.len() {
            out.extend([T::default(), T::default()]);
            continue;
        }
        let left = read(&data[base..], format);
        let right = if channels > 1 {
            read(&data[base + width..], format)
        } else {
            left
        };
        out.extend([left, right]);
    }
    out
}

/// `frames` frames of `channels`-channel interleaved `format` as stereo i16.
/// Short input is treated as silence rather than read past.
pub fn to_stereo_i16(data: &[u8], format: SampleFormat, channels: u16, frames: u32) -> Vec<i16> {
    stereo(data, format, channels, frames, sample)
}

/// `frames` frames of `channels`-channel interleaved `format` as stereo f32,
/// interleaved L R L R: the mixer's input. Short input is treated as silence
/// rather than read past.
pub fn to_stereo_f32(data: &[u8], format: SampleFormat, channels: u16, frames: u32) -> Vec<f32> {
    stereo(data, format, channels, frames, sample_f32)
}

/// Mixed f32 back to the encoder's i16, clamped. The same scale as
/// [`to_stereo_i16`]'s float path, so a float source comes out identical
/// whichever route it takes.
pub fn f32_to_i16(samples: &[f32]) -> Vec<i16> {
    samples
        .iter()
        .map(|v| (v.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes_of_i16(values: &[i16]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    fn bytes_of_f32(values: &[f32]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    #[test]
    fn float_stereo_is_scaled_and_clamped() {
        let data = bytes_of_f32(&[0.5, -1.0, 2.0, 0.0]);
        assert_eq!(
            to_stereo_i16(&data, SampleFormat::F32, 2, 2),
            vec![16_383, -32_767, 32_767, 0]
        );
    }

    #[test]
    fn surround_keeps_the_front_pair() {
        // Two frames of 6-channel 16-bit: FL FR C LFE SL SR.
        let data = bytes_of_i16(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]);
        assert_eq!(
            to_stereo_i16(&data, SampleFormat::I16, 6, 2),
            vec![1, 2, 7, 8]
        );
    }

    #[test]
    fn mono_is_doubled_and_24_bit_keeps_the_top_bytes() {
        // 0x123456 little-endian in three bytes; the top 16 bits are 0x1234.
        let data = [0x56, 0x34, 0x12];
        assert_eq!(
            to_stereo_i16(&data, SampleFormat::I24, 1, 1),
            vec![0x1234, 0x1234]
        );
    }

    #[test]
    fn short_input_is_silence() {
        assert_eq!(
            to_stereo_i16(&[], SampleFormat::F32, 2, 2),
            vec![0, 0, 0, 0]
        );
    }

    #[test]
    fn formats_have_a_width_and_a_name() {
        for (format, bytes, name) in [
            (SampleFormat::F32, 4, "32-bit float"),
            (SampleFormat::I16, 2, "16-bit PCM"),
            (SampleFormat::I24, 3, "24-bit PCM"),
            (SampleFormat::I32, 4, "32-bit PCM"),
        ] {
            assert_eq!((format.bytes(), format.name()), (bytes, name));
        }
    }

    #[test]
    fn only_the_encoder_rates_pass() {
        assert!(aac_rate_supported(48_000));
        assert!(aac_rate_supported(44_100));
        assert!(!aac_rate_supported(96_000));
    }

    #[test]
    fn the_f32_path_scales_each_integer_format_to_full_scale() {
        assert_eq!(
            to_stereo_f32(&bytes_of_i16(&[i16::MIN, 16_384]), SampleFormat::I16, 2, 1),
            vec![-1.0, 0.5]
        );
        // -0x400000 and 0x200000, little-endian in three bytes each.
        let data = [0x00, 0x00, 0xC0, 0x00, 0x00, 0x20];
        assert_eq!(to_stereo_f32(&data, SampleFormat::I24, 2, 1), vec![-0.5, 0.25]);
        let data: Vec<u8> = [i32::MIN, 1 << 29].iter().flat_map(|v| v.to_le_bytes()).collect();
        assert_eq!(to_stereo_f32(&data, SampleFormat::I32, 2, 1), vec![-1.0, 0.25]);
    }

    #[test]
    fn the_f32_path_passes_float_through_unclamped() {
        let data = bytes_of_f32(&[0.25, 1.5]);
        assert_eq!(to_stereo_f32(&data, SampleFormat::F32, 2, 1), vec![0.25, 1.5]);
    }

    #[test]
    fn the_f32_path_keeps_the_front_pair_doubles_mono_and_pads_short_input() {
        let data = bytes_of_i16(&[16_384, -16_384, 1, 1, 1, 1]);
        assert_eq!(to_stereo_f32(&data, SampleFormat::I16, 6, 1), vec![0.5, -0.5]);
        assert_eq!(
            to_stereo_f32(&bytes_of_i16(&[16_384]), SampleFormat::I16, 1, 2),
            vec![0.5, 0.5, 0.0, 0.0]
        );
    }

    #[test]
    fn both_paths_agree_on_a_float_source() {
        let data = bytes_of_f32(&[0.5, -1.0, 2.0, 0.0, -0.123, 0.999]);
        assert_eq!(
            f32_to_i16(&to_stereo_f32(&data, SampleFormat::F32, 2, 3)),
            to_stereo_i16(&data, SampleFormat::F32, 2, 3)
        );
    }
}
