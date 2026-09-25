//! The encoder MFTs on offer, and what driving any of them directly takes:
//! enumerating and activating a transform, media types, samples in and out.
//! The H.264 encoder is `h264.rs` and the AAC encoder `aac.rs`; both are built
//! on this.
//!
//! **No sink writer** (#239). Until then Media Foundation's sink writer chose,
//! loaded and fed the encoders and wrote the file, and it holds one audio
//! stream, so a file had track 0 only. Now the encoders are transforms this
//! code activates and drives itself, and their samples go to our own MP4
//! writer (`own::mux`), which holds every track (DEVELOPMENT.md §2.5).

use std::mem::ManuallyDrop;

use windows::Win32::Media::MediaFoundation::{
    IMFActivate, IMFAttributes, IMFCollection, IMFMediaType, IMFSample, IMFTransform,
    MF_E_TRANSFORM_NEED_MORE_INPUT, MF_E_TRANSFORM_STREAM_CHANGE, MFCreateAttributes,
    MFCreateMediaType, MFCreateMemoryBuffer, MFCreateSample, MFMediaType_Video,
    MFSampleExtension_CleanPoint, MFT_CATEGORY_VIDEO_ENCODER, MFT_ENUM_FLAG,
    MFT_ENUM_FLAG_ASYNCMFT, MFT_ENUM_FLAG_HARDWARE, MFT_ENUM_FLAG_LOCALMFT,
    MFT_ENUM_FLAG_SORTANDFILTER, MFT_ENUM_FLAG_SYNCMFT, MFT_ENUM_HARDWARE_URL_Attribute,
    MFT_ENUM_HARDWARE_VENDOR_ID_Attribute, MFT_FRIENDLY_NAME_Attribute, MFT_OUTPUT_DATA_BUFFER,
    MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES, MFT_OUTPUT_STREAM_PROVIDES_SAMPLES,
    MFT_REGISTER_TYPE_INFO, MFTEnumEx, MFVideoFormat_H264,
};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::core::{GUID, PWSTR};

use crate::recorder::own::select;
use crate::recorder::own::status::Loaded;

/// 8 Mbps, the libobs backend's `RateControl::CBR(8000)` (DEVELOPMENT.md
/// §2.4), so the two backends' files compare like for like.
pub const VIDEO_BITRATE: u32 = 8_000_000;
/// 160 kbps stereo AAC, DEVELOPMENT.md §2.4's figure and the libobs
/// backend's, in the bytes-per-second unit Microsoft's AAC encoder takes. It
/// accepts 12 000, 16 000, 20 000 and 24 000; this is 20 000.
pub const AAC_BYTES_PER_SECOND: u32 = 20_000;
/// Two seconds at 60 fps. The mux closes a fragment before each keyframe, so
/// this bounds how much a kill can cost.
pub const GOP_FRAMES: u32 = 120;

// --- Attributes and media types ----------------------------------------------

pub(super) fn string_attribute(attributes: &IMFAttributes, key: &GUID) -> Option<String> {
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

/// A `UINT32` attribute, or `None` if it is not set.
pub(super) fn u32_attribute(attributes: &IMFAttributes, key: &GUID) -> Option<u32> {
    // SAFETY: `key` is live for the call.
    unsafe { attributes.GetUINT32(key) }.ok()
}

/// A blob attribute, or `None` if it is not set.
pub(super) fn blob_attribute(attributes: &IMFAttributes, key: &GUID) -> Option<Vec<u8>> {
    // SAFETY: `key` is live for the call.
    let size = unsafe { attributes.GetBlobSize(key) }.ok()?;
    let mut blob = vec![0u8; size as usize];
    // SAFETY: `blob` is exactly the size the attribute reported.
    unsafe { attributes.GetBlob(key, &mut blob, None) }.ok()?;
    Some(blob)
}

pub(super) const fn pack(high: u32, low: u32) -> u64 {
    ((high as u64) << 32) | low as u64
}

pub(super) fn new_attributes(size: u32) -> Result<IMFAttributes, String> {
    let mut attributes: Option<IMFAttributes> = None;
    // SAFETY: `attributes` is a live out-parameter.
    unsafe { MFCreateAttributes(&mut attributes, size) }
        .map_err(|e| format!("MFCreateAttributes failed: {e}"))?;
    attributes.ok_or_else(|| "MFCreateAttributes returned nothing".to_string())
}

pub(super) fn new_media_type() -> Result<IMFMediaType, String> {
    // SAFETY: no arguments.
    unsafe { MFCreateMediaType() }.map_err(|e| format!("MFCreateMediaType failed: {e}"))
}

pub(super) fn set_guid(
    target: &IMFAttributes,
    key: &GUID,
    value: &GUID,
    what: &str,
) -> Result<(), String> {
    // SAFETY: both GUIDs are live for the call.
    unsafe { target.SetGUID(key, value) }.map_err(|e| format!("could not set {what}: {e}"))
}

pub(super) fn set_u32(target: &IMFAttributes, key: &GUID, value: u32, what: &str) -> Result<(), String> {
    // SAFETY: `key` is live for the call.
    unsafe { target.SetUINT32(key, value) }.map_err(|e| format!("could not set {what}: {e}"))
}

pub(super) fn set_u64(target: &IMFAttributes, key: &GUID, value: u64, what: &str) -> Result<(), String> {
    // SAFETY: `key` is live for the call.
    unsafe { target.SetUINT64(key, value) }.map_err(|e| format!("could not set {what}: {e}"))
}

/// Sets both of a transform's types, in whichever order it takes them.
///
/// Encoders disagree: Microsoft's H.264 encoder wants its output type first
/// (the input it can take depends on the profile and size it is to produce),
/// and its AAC encoder its input type first. `output_first` is the order
/// tried first; the other is tried if that fails, and the error names both.
pub(super) fn set_types(
    transform: &IMFTransform,
    input: &IMFMediaType,
    output: &IMFMediaType,
    output_first: bool,
) -> Result<(), String> {
    let set_input = || {
        // SAFETY: `input` is a complete media type; stream 0; no flags.
        unsafe { transform.SetInputType(0, input, 0) }.map_err(|e| format!("SetInputType: {e}"))
    };
    let set_output = || {
        // SAFETY: `output` is a complete media type; stream 0; no flags.
        unsafe { transform.SetOutputType(0, output, 0) }.map_err(|e| format!("SetOutputType: {e}"))
    };
    let in_order = |output_first: bool| {
        if output_first {
            set_output().and_then(|()| set_input())
        } else {
            set_input().and_then(|()| set_output())
        }
    };
    in_order(output_first).or_else(|first| {
        in_order(!output_first).map_err(|second| format!("{first}; the other way round, {second}"))
    })
}

// --- Enumeration and activation ------------------------------------------------

/// Every MFT of `category` Media Foundation offers for `flags`, taking
/// `input` or producing `output` where given, in its own order, each with
/// the plain description `select` reads.
pub(super) fn enumerate(
    category: GUID,
    flags: MFT_ENUM_FLAG,
    input: Option<MFT_REGISTER_TYPE_INFO>,
    output: Option<MFT_REGISTER_TYPE_INFO>,
) -> Result<Vec<(IMFActivate, select::Encoder)>, String> {
    let hardware = flags.0 & MFT_ENUM_FLAG_HARDWARE.0 != 0;
    let mut array: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut count = 0u32;
    // SAFETY: `input` and `output` outlive the call; the array and count are
    // live out-parameters, and the array is freed below.
    unsafe {
        MFTEnumEx(
            category,
            flags,
            input.as_ref().map(|t| t as *const _),
            output.as_ref().map(|t| t as *const _),
            &mut array,
            &mut count,
        )
    }
    .map_err(|e| format!("MFTEnumEx failed: {e}"))?;

    let mut found = Vec::new();
    for i in 0..count as usize {
        // SAFETY: MFTEnumEx returned `count` entries; reading one moves its
        // reference into `activate`, which releases it on drop.
        let activate = unsafe { array.add(i).read() };
        let Some(activate) = activate else { continue };
        let encoder = select::Encoder {
            name: string_attribute(&activate, &MFT_FRIENDLY_NAME_Attribute)
                .unwrap_or_else(|| "(no friendly name)".to_string()),
            vendor: string_attribute(&activate, &MFT_ENUM_HARDWARE_VENDOR_ID_Attribute),
            hardware,
        };
        found.push((activate, encoder));
    }
    if !array.is_null() {
        // SAFETY: the array itself was CoTaskMemAlloc'd by MFTEnumEx and
        // every entry has been moved out of it.
        unsafe { CoTaskMemFree(Some(array as *const _)) };
    }
    Ok(found)
}

const H264: MFT_REGISTER_TYPE_INFO =
    MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: MFVideoFormat_H264 };

fn hardware_flags() -> MFT_ENUM_FLAG {
    MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER
}

fn software_flags() -> MFT_ENUM_FLAG {
    MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_ASYNCMFT | MFT_ENUM_FLAG_LOCALMFT | MFT_ENUM_FLAG_SORTANDFILTER
}

/// Every H.264 encoder, hardware first, each group in Media Foundation's own
/// order: the list `select::rank` chooses from.
pub fn h264_encoders() -> Result<Vec<select::Encoder>, String> {
    let mut all: Vec<select::Encoder> =
        enumerate(MFT_CATEGORY_VIDEO_ENCODER, hardware_flags(), None, Some(H264))?
            .into_iter()
            .map(|(_, e)| e)
            .collect();
    all.extend(
        enumerate(MFT_CATEGORY_VIDEO_ENCODER, software_flags(), None, Some(H264))?
            .into_iter()
            .map(|(_, e)| e),
    );
    Ok(all)
}

/// The activation object for `wanted`, one of [`h264_encoders`]' entries,
/// found again by enumerating afresh.
///
/// Afresh rather than kept from the pre-warm, because an activation object
/// hands out the same transform until it is shut down, and a hardware
/// encoder's is shut down at the end of every recording: each recording
/// activates its own. Matched on the whole description, in Media
/// Foundation's order, so the first of two identical encoders is the one
/// `rank` chose.
pub(super) fn find_h264(wanted: &select::Encoder) -> Result<IMFActivate, String> {
    let flags = if wanted.hardware { hardware_flags() } else { software_flags() };
    enumerate(MFT_CATEGORY_VIDEO_ENCODER, flags, None, Some(H264))?
        .into_iter()
        .find(|(_, e)| e == wanted)
        .map(|(activate, _)| activate)
        .ok_or_else(|| {
            format!("the H.264 encoder {} is no longer offered by Media Foundation", wanted.name)
        })
}

/// What was activated, read from the transform's own attributes, and from
/// the activation object's where the transform does not say (Microsoft's
/// software MFT carries no friendly name once created). This is what
/// `status::check_loaded` checks against the ranking.
pub(super) fn loaded(transform: &IMFTransform, activate: &IMFActivate) -> Loaded {
    // SAFETY: `transform` is live.
    let own = unsafe { transform.GetAttributes() }.ok();
    let read = |key: &GUID| {
        own.as_ref()
            .and_then(|a| string_attribute(a, key))
            .or_else(|| string_attribute(activate, key))
    };
    Loaded {
        name: read(&MFT_FRIENDLY_NAME_Attribute),
        vendor: read(&MFT_ENUM_HARDWARE_VENDOR_ID_Attribute),
        url: read(&MFT_ENUM_HARDWARE_URL_Attribute),
    }
}

// --- Samples --------------------------------------------------------------------

/// A system-memory sample holding `bytes`, at `time` for `duration`, both in
/// 100 ns units.
pub(super) fn memory_sample(bytes: &[u8], time: i64, duration: i64) -> Result<IMFSample, String> {
    let len = u32::try_from(bytes.len()).map_err(|_| "a sample over 4 GiB".to_string())?;
    // SAFETY: no pointers.
    let buffer = unsafe { MFCreateMemoryBuffer(len.max(1)) }
        .map_err(|e| format!("MFCreateMemoryBuffer failed: {e}"))?;
    let mut data: *mut u8 = std::ptr::null_mut();
    // SAFETY: `data` is a live out-parameter; the buffer is unlocked below.
    unsafe { buffer.Lock(&mut data, None, None) }.map_err(|e| format!("Lock failed: {e}"))?;
    // SAFETY: the buffer holds at least `len` bytes (its max length), `bytes`
    // is exactly `len` long, and the two cannot overlap.
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), data, bytes.len()) };
    // SAFETY: pairs the Lock above.
    unsafe { buffer.Unlock() }.map_err(|e| format!("Unlock failed: {e}"))?;
    // SAFETY: `len` is within the buffer's max length.
    unsafe { buffer.SetCurrentLength(len) }
        .map_err(|e| format!("SetCurrentLength failed: {e}"))?;
    // SAFETY: no arguments.
    let sample = unsafe { MFCreateSample() }.map_err(|e| format!("MFCreateSample failed: {e}"))?;
    // SAFETY: `sample` and `buffer` are live.
    unsafe { sample.AddBuffer(&buffer) }.map_err(|e| format!("AddBuffer failed: {e}"))?;
    set_times(&sample, time, duration)?;
    Ok(sample)
}

pub(super) fn set_times(sample: &IMFSample, time: i64, duration: i64) -> Result<(), String> {
    // SAFETY: `sample` is live.
    unsafe { sample.SetSampleTime(time) }.map_err(|e| format!("SetSampleTime failed: {e}"))?;
    // SAFETY: `sample` is live.
    unsafe { sample.SetSampleDuration(duration) }
        .map_err(|e| format!("SetSampleDuration failed: {e}"))
}

/// One sample an encoder produced.
pub struct Encoded {
    /// 100 ns units, as the encoder stamped it.
    pub time: i64,
    pub duration: i64,
    /// `MFSampleExtension_CleanPoint`: a keyframe.
    pub keyframe: bool,
    /// An Annex B access unit, or a raw AAC frame.
    pub bytes: Vec<u8>,
}

fn read_sample(sample: &IMFSample) -> Result<Encoded, String> {
    // SAFETY: `sample` is live.
    let time = unsafe { sample.GetSampleTime() }.unwrap_or(0);
    // SAFETY: `sample` is live.
    let duration = unsafe { sample.GetSampleDuration() }.unwrap_or(0);
    let keyframe = u32_attribute(sample, &MFSampleExtension_CleanPoint).is_some_and(|v| v != 0);
    // SAFETY: `sample` is live.
    let buffer = unsafe { sample.ConvertToContiguousBuffer() }
        .map_err(|e| format!("ConvertToContiguousBuffer failed: {e}"))?;
    let mut data: *mut u8 = std::ptr::null_mut();
    let mut len = 0u32;
    // SAFETY: both out-parameters are live; unlocked below.
    unsafe { buffer.Lock(&mut data, None, Some(&mut len)) }
        .map_err(|e| format!("Lock (output) failed: {e}"))?;
    let bytes = if data.is_null() || len == 0 {
        Vec::new()
    } else {
        // SAFETY: the buffer is locked and holds `len` valid bytes at `data`.
        unsafe { std::slice::from_raw_parts(data, len as usize) }.to_vec()
    };
    // SAFETY: pairs the Lock above.
    unsafe { buffer.Unlock() }.map_err(|e| format!("Unlock (output) failed: {e}"))?;
    Ok(Encoded { time, duration, keyframe, bytes })
}

/// How a transform's output samples are allocated: by the transform, or by
/// the caller at `size` bytes.
#[derive(Clone, Copy, Debug)]
pub(super) struct OutputAlloc {
    provides: bool,
    size: u32,
}

impl OutputAlloc {
    /// A placeholder until the types are set and [`OutputAlloc::of`] can ask.
    pub(super) fn unset() -> OutputAlloc {
        OutputAlloc { provides: false, size: 0 }
    }

    /// Read once streaming can begin, which is when `GetOutputStreamInfo` is
    /// meaningful. `fallback` is the size to allocate if the transform says
    /// 0.
    pub(super) fn of(transform: &IMFTransform, fallback: u32) -> Result<OutputAlloc, String> {
        // SAFETY: `transform` is live; stream 0.
        let info = unsafe { transform.GetOutputStreamInfo(0) }
            .map_err(|e| format!("GetOutputStreamInfo failed: {e}"))?;
        let provides = info.dwFlags
            & (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0) as u32
            != 0;
        Ok(OutputAlloc { provides, size: if info.cbSize == 0 { fallback } else { info.cbSize } })
    }
}

/// What one `ProcessOutput` gave.
pub(super) enum Output {
    Sample(Encoded),
    /// `MF_E_TRANSFORM_NEED_MORE_INPUT`: nothing more until more goes in.
    NeedMoreInput,
    /// `MF_E_TRANSFORM_STREAM_CHANGE`: the output type was renegotiated;
    /// ask again.
    StreamChanged,
}

/// One `ProcessOutput` on stream 0.
pub(super) fn process_output(transform: &IMFTransform, alloc: OutputAlloc) -> Result<Output, String> {
    let sample = if alloc.provides {
        None
    } else {
        // SAFETY: no pointers.
        let buffer = unsafe { MFCreateMemoryBuffer(alloc.size) }
            .map_err(|e| format!("MFCreateMemoryBuffer (output) failed: {e}"))?;
        // SAFETY: no arguments.
        let sample =
            unsafe { MFCreateSample() }.map_err(|e| format!("MFCreateSample failed: {e}"))?;
        // SAFETY: both live.
        unsafe { sample.AddBuffer(&buffer) }.map_err(|e| format!("AddBuffer failed: {e}"))?;
        Some(sample)
    };
    let mut buffers = [MFT_OUTPUT_DATA_BUFFER {
        dwStreamID: 0,
        pSample: ManuallyDrop::new(sample),
        dwStatus: 0,
        pEvents: ManuallyDrop::new(None::<IMFCollection>),
    }];
    let mut status = 0u32;
    // SAFETY: one output buffer for stream 0, whose sample is ours or null
    // for a transform that provides its own; `status` is a live
    // out-parameter.
    let result = unsafe { transform.ProcessOutput(0, &mut buffers, &mut status) };
    let [buffer] = buffers;
    // Whatever the call left in the structure is ours to release: our own
    // sample back, or the transform's new one, and any events.
    let sample = ManuallyDrop::into_inner(buffer.pSample);
    drop(ManuallyDrop::into_inner(buffer.pEvents));
    match result {
        Ok(()) => match sample {
            Some(sample) => read_sample(&sample).map(Output::Sample),
            None => Err("ProcessOutput succeeded with no sample".to_string()),
        },
        Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => Ok(Output::NeedMoreInput),
        Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
            // The new output type is the transform's first offer.
            // SAFETY: `transform` is live; stream 0, type 0.
            let offered = unsafe { transform.GetOutputAvailableType(0, 0) }
                .map_err(|e| format!("GetOutputAvailableType after a stream change: {e}"))?;
            // SAFETY: `offered` is the transform's own complete type.
            unsafe { transform.SetOutputType(0, &offered, 0) }
                .map_err(|e| format!("SetOutputType after a stream change: {e}"))?;
            Ok(Output::StreamChanged)
        }
        Err(e) => Err(format!("ProcessOutput failed: {e}")),
    }
}

/// `ProcessOutput` until the transform needs more input, handing every
/// sample to `out`. The synchronous encoders' half of each input, and of the
/// drain. Bounded, so a transform that never says it needs more input cannot
/// hold the session thread.
pub(super) fn drain_outputs(
    transform: &IMFTransform,
    alloc: OutputAlloc,
    out: &mut Vec<Encoded>,
) -> Result<(), String> {
    for _ in 0..10_000 {
        match process_output(transform, alloc)? {
            Output::Sample(sample) => out.push(sample),
            Output::StreamChanged => {}
            Output::NeedMoreInput => return Ok(()),
        }
    }
    Err("the encoder produced output without end".to_string())
}
