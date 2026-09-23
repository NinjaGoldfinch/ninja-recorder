//! Endpoint samples to what the AAC encoder takes, with no Windows in it.
//!
//! A shared-mode endpoint delivers whatever its mix format is: usually 32-bit
//! float, sometimes 16- or 24-bit integer, with anywhere from one to eight
//! channels. Microsoft's AAC encoder takes 16-bit PCM, one, two or six
//! channels, at 44.1 or 48 kHz. The spike asks for stereo and keeps the first
//! two channels (a mono source is doubled), which is fine for measuring a
//! clock and is not a downmix anyone should ship.

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

/// `frames` frames of `channels`-channel interleaved `format` as stereo i16.
/// Short input is treated as silence rather than read past.
pub fn to_stereo_i16(data: &[u8], format: SampleFormat, channels: u16, frames: u32) -> Vec<i16> {
    let channels = usize::from(channels.max(1));
    let width = format.bytes();
    let stride = width * channels;
    let mut out = Vec::with_capacity(frames as usize * 2);
    for frame in 0..frames as usize {
        let base = frame * stride;
        if base + stride > data.len() {
            out.extend([0, 0]);
            continue;
        }
        let left = sample(&data[base..], format);
        let right = if channels > 1 {
            sample(&data[base + width..], format)
        } else {
            left
        };
        out.extend([left, right]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn float_stereo_is_scaled_and_clamped() {
        let mut data = Vec::new();
        for v in [0.5f32, -1.0, 2.0, 0.0] {
            data.extend(v.to_le_bytes());
        }
        assert_eq!(
            to_stereo_i16(&data, SampleFormat::F32, 2, 2),
            vec![16_383, -32_767, 32_767, 0]
        );
    }

    #[test]
    fn surround_keeps_the_front_pair() {
        // Two frames of 6-channel 16-bit: FL FR C LFE SL SR.
        let frames: [[i16; 6]; 2] = [[1, 2, 3, 4, 5, 6], [7, 8, 9, 10, 11, 12]];
        let data: Vec<u8> = frames
            .iter()
            .flatten()
            .flat_map(|v| v.to_le_bytes())
            .collect();
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
    fn only_the_encoder_rates_pass() {
        assert!(aac_rate_supported(48_000));
        assert!(aac_rate_supported(44_100));
        assert!(!aac_rate_supported(96_000));
    }
}
