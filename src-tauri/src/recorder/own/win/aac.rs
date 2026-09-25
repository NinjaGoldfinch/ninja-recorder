//! One AAC encoder MFT per written audio track (#239).
//!
//! Media Foundation's AAC encoder is synchronous: 16-bit stereo PCM goes in,
//! and raw AAC-LC frames of 1024 samples come out, `MF_MT_AAC_PAYLOAD_TYPE`
//! 0, which is what an MP4 `mp4a` track carries (no ADTS header). 160 kbps,
//! 48 kHz stereo, DEVELOPMENT.md §2.4's figures and the libobs backend's.
//!
//! Each track has its own encoder, fed its own mix (`own::mix::Mixdown`), so a
//! stem is encoded exactly as track 0 is and nothing is shared between them
//! but the rate.

use windows::Win32::Media::MediaFoundation::{
    IMFActivate, IMFMediaType, IMFShutdown, IMFTransform, MF_E_NOTACCEPTING,
    MF_MT_AAC_AUDIO_PROFILE_LEVEL_INDICATION, MF_MT_AAC_PAYLOAD_TYPE,
    MF_MT_AUDIO_AVG_BYTES_PER_SECOND, MF_MT_AUDIO_BITS_PER_SAMPLE, MF_MT_AUDIO_BLOCK_ALIGNMENT,
    MF_MT_AUDIO_NUM_CHANNELS, MF_MT_AUDIO_SAMPLES_PER_SECOND, MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE,
    MFAudioFormat_AAC, MFAudioFormat_PCM, MFMediaType_Audio, MFT_CATEGORY_AUDIO_ENCODER,
    MFT_ENUM_FLAG_LOCALMFT, MFT_ENUM_FLAG_SORTANDFILTER, MFT_ENUM_FLAG_SYNCMFT,
    MFT_MESSAGE_COMMAND_DRAIN, MFT_MESSAGE_NOTIFY_BEGIN_STREAMING,
    MFT_MESSAGE_NOTIFY_END_OF_STREAM, MFT_MESSAGE_NOTIFY_END_STREAMING,
    MFT_MESSAGE_NOTIFY_START_OF_STREAM, MFT_MESSAGE_TYPE, MFT_REGISTER_TYPE_INFO,
};
use windows::core::{GUID, Interface};

use super::encode::{
    self, AAC_BYTES_PER_SECOND, Encoded, OutputAlloc, new_media_type, set_guid, set_u32,
};
use crate::recorder::own::clock::HNS_PER_SECOND;

/// AAC-LC, level 2: `0x29`, what the sink writer was told before #239.
const AAC_LC_L2: u32 = 0x29;

/// Stereo 16-bit at `rate`: the PCM the encoder takes, or with
/// `MFAudioFormat_AAC` what it writes.
fn audio_type(subtype: &GUID, rate: u32) -> Result<IMFMediaType, String> {
    let media_type = new_media_type()?;
    set_guid(&media_type, &MF_MT_MAJOR_TYPE, &MFMediaType_Audio, "the major type")?;
    set_guid(&media_type, &MF_MT_SUBTYPE, subtype, "the subtype")?;
    set_u32(&media_type, &MF_MT_AUDIO_SAMPLES_PER_SECOND, rate, "the sample rate")?;
    set_u32(&media_type, &MF_MT_AUDIO_NUM_CHANNELS, 2, "the channel count")?;
    set_u32(&media_type, &MF_MT_AUDIO_BITS_PER_SAMPLE, 16, "the sample size")?;
    if *subtype == MFAudioFormat_AAC {
        set_u32(
            &media_type,
            &MF_MT_AUDIO_AVG_BYTES_PER_SECOND,
            AAC_BYTES_PER_SECOND,
            "the AAC bitrate",
        )?;
        // Raw AAC, which is what MP4 carries: no ADTS header on each frame.
        set_u32(&media_type, &MF_MT_AAC_PAYLOAD_TYPE, 0, "the AAC payload type")?;
        set_u32(
            &media_type,
            &MF_MT_AAC_AUDIO_PROFILE_LEVEL_INDICATION,
            AAC_LC_L2,
            "the AAC profile",
        )?;
    } else {
        set_u32(&media_type, &MF_MT_AUDIO_BLOCK_ALIGNMENT, 4, "the block alignment")?;
        set_u32(&media_type, &MF_MT_AUDIO_AVG_BYTES_PER_SECOND, rate * 4, "the byte rate")?;
    }
    Ok(media_type)
}

fn message(transform: &IMFTransform, what: &str, message: MFT_MESSAGE_TYPE) -> Result<(), String> {
    // SAFETY: `transform` is live; every message sent here takes 0.
    unsafe { transform.ProcessMessage(message, 0) }.map_err(|e| format!("{what} failed: {e}"))
}

/// One track's AAC encoder, streaming.
pub struct AacEncoder {
    transform: IMFTransform,
    activate: IMFActivate,
    alloc: OutputAlloc,
    rate: u32,
    ended: bool,
}

impl AacEncoder {
    /// The first AAC encoder Media Foundation offers, set up for 160 kbps
    /// stereo at `rate`.
    pub fn create(rate: u32) -> Result<AacEncoder, String> {
        let pcm = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Audio, guidSubtype: MFAudioFormat_PCM };
        let aac = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Audio, guidSubtype: MFAudioFormat_AAC };
        let (activate, _) = encode::enumerate(
            MFT_CATEGORY_AUDIO_ENCODER,
            MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_LOCALMFT | MFT_ENUM_FLAG_SORTANDFILTER,
            Some(pcm),
            Some(aac),
        )?
        .into_iter()
        .next()
        .ok_or("Media Foundation offers no AAC encoder")?;
        // SAFETY: `activate` is live; the transform is ours until shut down.
        let transform: IMFTransform = unsafe { activate.ActivateObject() }
            .map_err(|e| format!("activating the AAC encoder failed: {e}"))?;
        let mut encoder =
            AacEncoder { transform, activate, alloc: OutputAlloc::unset(), rate, ended: false };
        // Input first: the AAC encoder offers its outputs for the input it has.
        let input = audio_type(&MFAudioFormat_PCM, rate)?;
        let output = audio_type(&MFAudioFormat_AAC, rate)?;
        encode::set_types(&encoder.transform, &input, &output, false)
            .map_err(|e| format!("the AAC encoder refused 160 kbps stereo at {rate} Hz: {e}"))?;
        // An AAC frame is at most 768 bytes a channel; the encoder normally
        // says so itself.
        encoder.alloc = OutputAlloc::of(&encoder.transform, 8 * 1024)?;
        message(&encoder.transform, "NOTIFY_BEGIN_STREAMING", MFT_MESSAGE_NOTIFY_BEGIN_STREAMING)?;
        message(&encoder.transform, "NOTIFY_START_OF_STREAM", MFT_MESSAGE_NOTIFY_START_OF_STREAM)?;
        Ok(encoder)
    }

    /// Encodes `pcm` (stereo i16, interleaved), which starts at sample
    /// `position` of the track, and hands every finished AAC frame to `out`.
    /// Positions are contiguous from 0 (`own::mix` guarantees it), so the
    /// sample times are too.
    pub fn encode(&mut self, pcm: &[i16], position: u64, out: &mut Vec<Encoded>) -> Result<(), String> {
        if pcm.is_empty() {
            return Ok(());
        }
        if self.ended {
            return Err("the AAC encoder has been drained".to_string());
        }
        let at = |samples: u64| {
            (i128::from(samples) * i128::from(HNS_PER_SECOND) / i128::from(self.rate)) as i64
        };
        let time = at(position);
        let end = at(position + (pcm.len() / 2) as u64);
        // SAFETY: an i16 slice is its own bytes, twice as many, in the
        // little-endian order PCM is; the view lives only for the copy.
        let bytes = unsafe { std::slice::from_raw_parts(pcm.as_ptr().cast::<u8>(), pcm.len() * 2) };
        let sample = encode::memory_sample(bytes, time, end - time)?;
        for _ in 0..2 {
            // SAFETY: `sample` is live; stream 0, no flags.
            match unsafe { self.transform.ProcessInput(0, &sample, 0) } {
                Ok(()) => return encode::drain_outputs(&self.transform, self.alloc, out),
                Err(e) if e.code() == MF_E_NOTACCEPTING => {
                    encode::drain_outputs(&self.transform, self.alloc, out)?;
                }
                Err(e) => return Err(format!("the AAC encoder's ProcessInput failed: {e}")),
            }
        }
        Err("the AAC encoder would not accept input even after its output was taken".to_string())
    }

    /// Takes the frames still inside the encoder, the end of the track.
    pub fn drain(&mut self, out: &mut Vec<Encoded>) -> Result<(), String> {
        if self.ended {
            return Ok(());
        }
        self.ended = true;
        message(&self.transform, "NOTIFY_END_OF_STREAM", MFT_MESSAGE_NOTIFY_END_OF_STREAM)?;
        message(&self.transform, "COMMAND_DRAIN", MFT_MESSAGE_COMMAND_DRAIN)?;
        encode::drain_outputs(&self.transform, self.alloc, out)?;
        message(&self.transform, "NOTIFY_END_STREAMING", MFT_MESSAGE_NOTIFY_END_STREAMING)
    }
}

impl Drop for AacEncoder {
    fn drop(&mut self) {
        if let Ok(shutdown) = self.transform.cast::<IMFShutdown>() {
            // SAFETY: `shutdown` is live; nothing uses the transform after.
            let _ = unsafe { shutdown.Shutdown() };
        }
        // SAFETY: `activate` made the transform; nothing uses it after this.
        let _ = unsafe { self.activate.ShutdownObject() };
    }
}
