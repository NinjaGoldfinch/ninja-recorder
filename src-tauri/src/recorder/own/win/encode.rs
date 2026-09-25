//! H.264 encoder enumeration, and the Media Foundation sink writer that turns
//! textures and PCM into a fragmented MP4. Ported from
//! `spikes/p0c-video/src/win/encode.rs`.
//!
//! **The sink writer is this piece's path, not the final one.** Media
//! Foundation's MP4 sinks hold one audio stream, which is the Game preset's
//! one track (#237) and no more, so WS1.6.7 (#239) replaces this with the
//! encoders driven directly and `mp4::write` doing the muxing
//! (DEVELOPMENT.md §2.5). Until then this is the path the spike proved.

use std::path::Path;
use std::sync::atomic::Ordering;

use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11Texture2D};
use windows::Win32::Media::MediaFoundation::{
    CODECAPI_AVEncCommonMeanBitRate, CODECAPI_AVEncCommonRateControlMode,
    CODECAPI_AVEncMPVGOPSize, IMF2DBuffer, IMFActivate, IMFAttributes, IMFDXGIDeviceManager,
    IMFMediaType, IMFSample, IMFSinkWriter, IMFSinkWriterEx, IMFTransform,
    MF_MT_AAC_AUDIO_PROFILE_LEVEL_INDICATION, MF_MT_AAC_PAYLOAD_TYPE,
    MF_MT_AUDIO_AVG_BYTES_PER_SECOND, MF_MT_AUDIO_BITS_PER_SAMPLE, MF_MT_AUDIO_BLOCK_ALIGNMENT,
    MF_MT_AUDIO_NUM_CHANNELS, MF_MT_AUDIO_SAMPLES_PER_SECOND, MF_MT_AVG_BITRATE,
    MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE,
    MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SUBTYPE, MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS,
    MF_SINK_WRITER_D3D_MANAGER, MF_TRANSCODE_CONTAINERTYPE, MFAudioFormat_AAC,
    MFAudioFormat_PCM, MFCreateAttributes, MFCreateDXGIDeviceManager, MFCreateDXGISurfaceBuffer,
    MFCreateMediaType, MFCreateMemoryBuffer, MFCreateSample, MFCreateSinkWriterFromURL,
    MFCreateTrackedSample, MFMediaType_Audio, MFMediaType_Video,
    MFT_CATEGORY_VIDEO_ENCODER, MFT_ENUM_FLAG, MFT_ENUM_FLAG_ASYNCMFT, MFT_ENUM_FLAG_HARDWARE,
    MFT_ENUM_FLAG_LOCALMFT, MFT_ENUM_FLAG_SORTANDFILTER, MFT_ENUM_FLAG_SYNCMFT,
    MFT_ENUM_HARDWARE_URL_Attribute, MFT_ENUM_HARDWARE_VENDOR_ID_Attribute,
    MFT_FRIENDLY_NAME_Attribute, MFT_REGISTER_TYPE_INFO, MFTEnumEx,
    MFTranscodeContainerType_FMPEG4, MFVideoFormat_H264, MFVideoFormat_RGB32,
    MFVideoInterlace_Progressive, eAVEncCommonRateControlMode_CBR,
};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::core::{GUID, HSTRING, Interface, PWSTR};

use super::capture::Slot;
use crate::recorder::own::clock::HNS_PER_SECOND;
use crate::recorder::own::select;
use crate::recorder::own::status::Loaded;

/// 8 Mbps, the libobs backend's `RateControl::CBR(8000)` (DEVELOPMENT.md
/// §2.4), so the two backends' files compare like for like.
pub const VIDEO_BITRATE: u32 = 8_000_000;
/// 160 kbps stereo AAC, DEVELOPMENT.md §2.4's figure and the libobs
/// backend's, in the bytes-per-second unit Microsoft's AAC encoder takes. It
/// accepts 12 000, 16 000, 20 000 and 24 000; this is 20 000.
pub const AAC_BYTES_PER_SECOND: u32 = 20_000;
/// Two seconds at 60 fps. The fragmented sink closes a fragment at a
/// keyframe, so this bounds how much a kill can cost.
pub const GOP_FRAMES: u32 = 120;

// --- Enumeration -----------------------------------------------------------

fn string_attribute(attributes: &IMFAttributes, key: &GUID) -> Option<String> {
    let mut value = PWSTR::null();
    let mut len = 0u32;
    // SAFETY: both out-parameters are live; on success the string is ours
    // to free with CoTaskMemFree.
    unsafe { attributes.GetAllocatedString(key, &mut value, &mut len) }.ok()?;
    // SAFETY: GetAllocatedString returned a NUL-terminated wide string.
    let text = unsafe { value.to_string() }.ok();
    // SAFETY: the buffer was allocated by Media Foundation with CoTaskMemAlloc.
    unsafe { CoTaskMemFree(Some(value.0 as *const _)) };
    text
}

fn enumerate(flags: MFT_ENUM_FLAG, hardware: bool) -> Result<Vec<select::Encoder>, String> {
    let output =
        MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: MFVideoFormat_H264 };
    let mut array: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut count = 0u32;
    // SAFETY: `output` outlives the call; the array and count are live
    // out-parameters, and the array is freed below.
    unsafe {
        MFTEnumEx(MFT_CATEGORY_VIDEO_ENCODER, flags, None, Some(&output), &mut array, &mut count)
    }
    .map_err(|e| format!("MFTEnumEx failed: {e}"))?;

    let mut found = Vec::new();
    for i in 0..count as usize {
        // SAFETY: MFTEnumEx returned `count` entries; reading one moves its
        // reference into `activate`, which releases it on drop.
        let activate = unsafe { array.add(i).read() };
        let Some(activate) = activate else { continue };
        found.push(select::Encoder {
            name: string_attribute(&activate, &MFT_FRIENDLY_NAME_Attribute)
                .unwrap_or_else(|| "(no friendly name)".to_string()),
            vendor: string_attribute(&activate, &MFT_ENUM_HARDWARE_VENDOR_ID_Attribute),
            hardware,
        });
    }
    if !array.is_null() {
        // SAFETY: the array itself was CoTaskMemAlloc'd by MFTEnumEx and
        // every entry has been moved out of it.
        unsafe { CoTaskMemFree(Some(array as *const _)) };
    }
    Ok(found)
}

/// Every H.264 encoder, hardware first, each group in Media Foundation's own
/// order: the list `select::rank` chooses from.
pub fn h264_encoders() -> Result<Vec<select::Encoder>, String> {
    let mut all = enumerate(MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER, true)?;
    all.extend(enumerate(
        MFT_ENUM_FLAG_SYNCMFT
            | MFT_ENUM_FLAG_ASYNCMFT
            | MFT_ENUM_FLAG_LOCALMFT
            | MFT_ENUM_FLAG_SORTANDFILTER,
        false,
    )?);
    Ok(all)
}

// --- The sink writer -------------------------------------------------------

const fn pack(high: u32, low: u32) -> u64 {
    ((high as u64) << 32) | low as u64
}

fn new_attributes(size: u32) -> Result<IMFAttributes, String> {
    let mut attributes: Option<IMFAttributes> = None;
    // SAFETY: `attributes` is a live out-parameter.
    unsafe { MFCreateAttributes(&mut attributes, size) }
        .map_err(|e| format!("MFCreateAttributes failed: {e}"))?;
    attributes.ok_or_else(|| "MFCreateAttributes returned nothing".to_string())
}

fn set_guid(target: &IMFAttributes, key: &GUID, value: &GUID, what: &str) -> Result<(), String> {
    // SAFETY: both GUIDs are live for the call.
    unsafe { target.SetGUID(key, value) }.map_err(|e| format!("could not set {what}: {e}"))
}

fn set_u32(target: &IMFAttributes, key: &GUID, value: u32, what: &str) -> Result<(), String> {
    // SAFETY: `key` is live for the call.
    unsafe { target.SetUINT32(key, value) }.map_err(|e| format!("could not set {what}: {e}"))
}

fn set_u64(target: &IMFAttributes, key: &GUID, value: u64, what: &str) -> Result<(), String> {
    // SAFETY: `key` is live for the call.
    unsafe { target.SetUINT64(key, value) }.map_err(|e| format!("could not set {what}: {e}"))
}

fn video_type(
    subtype: &GUID,
    width: u32,
    height: u32,
    fps: u32,
    bitrate: Option<u32>,
) -> Result<IMFMediaType, String> {
    // SAFETY: no arguments.
    let media_type =
        unsafe { MFCreateMediaType() }.map_err(|e| format!("MFCreateMediaType failed: {e}"))?;
    set_guid(&media_type, &MF_MT_MAJOR_TYPE, &MFMediaType_Video, "the major type")?;
    set_guid(&media_type, &MF_MT_SUBTYPE, subtype, "the subtype")?;
    set_u32(
        &media_type,
        &MF_MT_INTERLACE_MODE,
        MFVideoInterlace_Progressive.0 as u32,
        "the interlace mode",
    )?;
    // Packed pairs in one 64-bit attribute, high half first. Setting them as
    // two 32-bit values fails at AddStream with an error that names neither.
    set_u64(&media_type, &MF_MT_FRAME_SIZE, pack(width, height), "the frame size")?;
    set_u64(&media_type, &MF_MT_FRAME_RATE, pack(fps, 1), "the frame rate")?;
    set_u64(&media_type, &MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1), "the pixel aspect ratio")?;
    if let Some(bitrate) = bitrate {
        set_u32(&media_type, &MF_MT_AVG_BITRATE, bitrate, "the bitrate")?;
    }
    Ok(media_type)
}

/// Stereo 16-bit at `rate`: the PCM the AAC encoder takes, or with
/// `MFAudioFormat_AAC` the stream it writes.
fn audio_type(subtype: &GUID, rate: u32) -> Result<IMFMediaType, String> {
    // SAFETY: no arguments.
    let media_type =
        unsafe { MFCreateMediaType() }.map_err(|e| format!("MFCreateMediaType failed: {e}"))?;
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
        // Raw AAC, which is what MP4 carries; 0x29 is AAC-LC level 2.
        set_u32(&media_type, &MF_MT_AAC_PAYLOAD_TYPE, 0, "the AAC payload type")?;
        set_u32(&media_type, &MF_MT_AAC_AUDIO_PROFILE_LEVEL_INDICATION, 0x29, "the AAC profile")?;
    } else {
        set_u32(&media_type, &MF_MT_AUDIO_BLOCK_ALIGNMENT, 4, "the block alignment")?;
        set_u32(&media_type, &MF_MT_AUDIO_AVG_BYTES_PER_SECOND, rate * 4, "the byte rate")?;
    }
    Ok(media_type)
}

/// A sink writer with one H.264 stream and at most one AAC stream, writing
/// a fragmented MP4.
pub struct Sink {
    writer: IMFSinkWriter,
    video: u32,
    /// The AAC stream's index and its sample rate, when there is one.
    audio: Option<(u32, u32)>,
}

impl Sink {
    /// Creates the file at `out` and the encoder behind it.
    ///
    /// `hardware` is whether the sink writer may load a hardware encoder,
    /// which is the ranked `Choice`: a hardware choice allows it, a software
    /// fallback forbids it so the software MFT is what loads. Which encoder
    /// actually loaded is [`Sink::loaded`]'s answer, read after
    /// [`Sink::begin`].
    ///
    /// `audio_rate` adds an AAC stream at 160 kbps, fed stereo i16 at that
    /// rate through [`Sink::write_audio`]; `None` writes video only.
    pub fn create(
        out: &Path,
        width: u32,
        height: u32,
        fps: u32,
        device: &ID3D11Device,
        hardware: bool,
        audio_rate: Option<u32>,
    ) -> Result<Sink, String> {
        // The device manager is what lets the encoder read the captured
        // texture where it already is.
        let mut reset_token = 0u32;
        let mut manager: Option<IMFDXGIDeviceManager> = None;
        // SAFETY: both out-parameters are live.
        unsafe { MFCreateDXGIDeviceManager(&mut reset_token, &mut manager) }
            .map_err(|e| format!("MFCreateDXGIDeviceManager failed: {e}"))?;
        let manager = manager.ok_or("MFCreateDXGIDeviceManager returned nothing")?;
        // SAFETY: the device is live and the token is the one just issued.
        unsafe { manager.ResetDevice(device, reset_token) }
            .map_err(|e| format!("ResetDevice failed: {e}"))?;

        let attributes = new_attributes(3)?;
        // SAFETY: `manager` is live and the attribute store takes a reference.
        unsafe { attributes.SetUnknown(&MF_SINK_WRITER_D3D_MANAGER, &manager) }
            .map_err(|e| format!("could not attach the D3D manager: {e}"))?;
        set_u32(
            &attributes,
            &MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS,
            u32::from(hardware),
            "whether hardware transforms are allowed",
        )?;
        // **Fragmented MP4, which is what makes a killed recording playable.**
        // An ordinary MP4 writes its index at the end, so a killed file has
        // no `moov` and plays nowhere. Fragmented MP4 writes the header first
        // and self-contained fragments after it. Selecting the container this
        // way makes the sink writer build the sink with
        // `MFCreateFMPEG4MediaSink` itself.
        set_guid(
            &attributes,
            &MF_TRANSCODE_CONTAINERTYPE,
            &MFTranscodeContainerType_FMPEG4,
            "fragmented MP4",
        )?;

        // SAFETY: the URL is a live HSTRING, no byte stream, live attributes.
        let writer = unsafe {
            MFCreateSinkWriterFromURL(&HSTRING::from(out.as_os_str()), None, &attributes)
        }
        .map_err(|e| format!("MFCreateSinkWriterFromURL failed: {e}"))?;

        let target = video_type(&MFVideoFormat_H264, width, height, fps, Some(VIDEO_BITRATE))?;
        // SAFETY: `target` is a complete media type.
        let video = unsafe { writer.AddStream(&target) }
            .map_err(|e| format!("AddStream (video) failed: {e}"))?;
        let source = video_type(&MFVideoFormat_RGB32, width, height, fps, None)?;

        // Handed to the encoder's ICodecAPI by the sink writer. CBR at the
        // libobs backend's bitrate, for parity (DEVELOPMENT.md §2.4), and a
        // keyframe every two seconds.
        let params = new_attributes(3)?;
        set_u32(
            &params,
            &CODECAPI_AVEncCommonRateControlMode,
            eAVEncCommonRateControlMode_CBR.0 as u32,
            "CBR rate control",
        )?;
        set_u32(&params, &CODECAPI_AVEncCommonMeanBitRate, VIDEO_BITRATE, "the mean bitrate")?;
        set_u32(&params, &CODECAPI_AVEncMPVGOPSize, GOP_FRAMES, "the GOP size")?;
        // SAFETY: both are complete; `params` is an ICodecAPI property store.
        unsafe { writer.SetInputMediaType(video, &source, &params) }.map_err(|e| {
            format!(
                "SetInputMediaType (video) failed: {e}. This is where a machine with no usable \
                 H.264 encoder for this input says so"
            )
        })?;

        let audio = match audio_rate {
            None => None,
            Some(rate) => {
                let target = audio_type(&MFAudioFormat_AAC, rate)?;
                // SAFETY: `target` is a complete media type.
                let stream = unsafe { writer.AddStream(&target) }
                    .map_err(|e| format!("AddStream (AAC) failed: {e}"))?;
                let source = audio_type(&MFAudioFormat_PCM, rate)?;
                // SAFETY: `source` is complete; no encoder parameters.
                unsafe { writer.SetInputMediaType(stream, &source, None) }
                    .map_err(|e| format!("SetInputMediaType (AAC) failed: {e}"))?;
                Some((stream, rate))
            }
        };

        Ok(Sink { writer, video, audio })
    }

    /// Whether the file has an audio track.
    pub fn has_audio(&self) -> bool {
        self.audio.is_some()
    }

    /// Hands `pcm` (stereo i16, interleaved) to the AAC encoder, starting at
    /// sample `position` of the audio track. Positions are contiguous from 0
    /// (`feed::Feed` guarantees it), so the sample times are too.
    pub fn write_audio(&self, pcm: &[i16], position: u64) -> Result<(), String> {
        let Some((stream, rate)) = self.audio else {
            return Ok(());
        };
        if pcm.is_empty() {
            return Ok(());
        }
        let bytes = pcm.len() * 2;
        // SAFETY: no pointers.
        let buffer = unsafe { MFCreateMemoryBuffer(bytes as u32) }
            .map_err(|e| format!("MFCreateMemoryBuffer failed: {e}"))?;
        let mut data: *mut u8 = std::ptr::null_mut();
        // SAFETY: `data` is a live out-parameter; the buffer is unlocked below.
        unsafe { buffer.Lock(&mut data, None, None) }.map_err(|e| format!("Lock failed: {e}"))?;
        // SAFETY: the buffer holds at least `bytes` bytes (its max length),
        // `pcm` is exactly `bytes` long, and the two cannot overlap.
        unsafe { std::ptr::copy_nonoverlapping(pcm.as_ptr().cast::<u8>(), data, bytes) };
        // SAFETY: pairs the Lock above.
        unsafe { buffer.Unlock() }.map_err(|e| format!("Unlock failed: {e}"))?;
        // SAFETY: `bytes` is within the buffer's max length.
        unsafe { buffer.SetCurrentLength(bytes as u32) }
            .map_err(|e| format!("SetCurrentLength failed: {e}"))?;

        let at = |samples: u64| {
            (i128::from(samples) * i128::from(HNS_PER_SECOND) / i128::from(rate)) as i64
        };
        let time = at(position);
        let end = at(position + (pcm.len() / 2) as u64);
        // SAFETY: no arguments.
        let sample =
            unsafe { MFCreateSample() }.map_err(|e| format!("MFCreateSample failed: {e}"))?;
        // SAFETY: `sample` and `buffer` are live.
        unsafe { sample.AddBuffer(&buffer) }.map_err(|e| format!("AddBuffer failed: {e}"))?;
        // SAFETY: `sample` is live.
        unsafe { sample.SetSampleTime(time) }.map_err(|e| format!("SetSampleTime failed: {e}"))?;
        // SAFETY: `sample` is live.
        unsafe { sample.SetSampleDuration(end - time) }
            .map_err(|e| format!("SetSampleDuration failed: {e}"))?;
        // SAFETY: the stream index came from AddStream and writing has begun.
        unsafe { self.writer.WriteSample(stream, &sample) }
            .map_err(|e| format!("WriteSample (audio) failed: {e}"))
    }

    /// Starts writing: the file's header goes out, and the encoder is
    /// loaded.
    pub fn begin(&self) -> Result<(), String> {
        // SAFETY: every stream has its input type set.
        unsafe { self.writer.BeginWriting() }.map_err(|e| format!("BeginWriting failed: {e}"))
    }

    /// What the sink writer actually loaded as the video encoder, read from
    /// the transform's own attributes. Only meaningful after [`Sink::begin`].
    pub fn loaded(&self) -> Result<Loaded, String> {
        let ex: IMFSinkWriterEx = self
            .writer
            .cast()
            .map_err(|e| format!("the sink writer has no IMFSinkWriterEx: {e}"))?;
        let mut loaded = Loaded::default();
        for index in 0.. {
            let mut category = GUID::zeroed();
            let mut transform: Option<IMFTransform> = None;
            // SAFETY: both out-parameters are live; an index past the chain
            // is an error, which ends the loop.
            if unsafe {
                ex.GetTransformForStream(self.video, index, Some(&mut category), &mut transform)
            }
            .is_err()
            {
                break;
            }
            if category == MFT_CATEGORY_VIDEO_ENCODER
                && let Some(transform) = transform
                // SAFETY: `transform` is live.
                && let Ok(attributes) = unsafe { transform.GetAttributes() }
            {
                loaded.name = string_attribute(&attributes, &MFT_FRIENDLY_NAME_Attribute);
                loaded.vendor =
                    string_attribute(&attributes, &MFT_ENUM_HARDWARE_VENDOR_ID_Attribute);
                loaded.url = string_attribute(&attributes, &MFT_ENUM_HARDWARE_URL_Attribute);
            }
        }
        Ok(loaded)
    }

    /// Hands one tick to the encoder: the slot's texture, wrapped, not
    /// copied. The sample is tracked, so the slot knows when the encoder
    /// has let go of it.
    pub fn write(&self, slot: &Slot, time: i64, duration: i64) -> Result<(), String> {
        // SAFETY: the texture is live; subresource 0, not bottom-up.
        let buffer =
            unsafe { MFCreateDXGISurfaceBuffer(&ID3D11Texture2D::IID, &slot.texture, 0, false) }
                .map_err(|e| format!("MFCreateDXGISurfaceBuffer failed: {e}"))?;
        // A DXGI buffer starts with a current length of zero, and some
        // encoders treat that as an empty frame. Its contiguous length is the
        // real size.
        if let Ok(two_d) = buffer.cast::<IMF2DBuffer>()
            // SAFETY: `two_d` is live.
            && let Ok(length) = unsafe { two_d.GetContiguousLength() }
        {
            // SAFETY: `buffer` is live; the length is its own.
            let _ = unsafe { buffer.SetCurrentLength(length) };
        }

        // SAFETY: no arguments.
        let tracked = unsafe { MFCreateTrackedSample() }
            .map_err(|e| format!("MFCreateTrackedSample failed: {e}"))?;
        slot.in_flight.fetch_add(1, Ordering::AcqRel);
        // SAFETY: the callback is live for as long as the slot is, which
        // outlives every sample written from it (the sink is finalized first).
        if let Err(e) = unsafe { tracked.SetAllocator(&slot.callback, None) } {
            slot.in_flight.fetch_sub(1, Ordering::AcqRel);
            return Err(format!("SetAllocator failed: {e}"));
        }
        let sample: IMFSample =
            tracked.cast().map_err(|e| format!("a tracked sample is not an IMFSample: {e}"))?;
        // SAFETY: `sample` and `buffer` are live.
        unsafe { sample.AddBuffer(&buffer) }.map_err(|e| format!("AddBuffer failed: {e}"))?;
        // SAFETY: `sample` is live.
        unsafe { sample.SetSampleTime(time) }.map_err(|e| format!("SetSampleTime failed: {e}"))?;
        // SAFETY: `sample` is live.
        unsafe { sample.SetSampleDuration(duration) }
            .map_err(|e| format!("SetSampleDuration failed: {e}"))?;
        // SAFETY: the stream index came from AddStream and writing has begun.
        unsafe { self.writer.WriteSample(self.video, &sample) }
            .map_err(|e| format!("WriteSample (video) failed: {e}"))
    }

    /// Drains the encoder and closes the file. Consumes the sink, so nothing
    /// can be written after it.
    pub fn finalize(self) -> Result<(), String> {
        // SAFETY: writing has begun; every sample has been handed over.
        unsafe { self.writer.Finalize() }.map_err(|e| format!("Finalize failed: {e}"))
    }
}
