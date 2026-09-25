//! The H.264 encoder MFT, driven directly (#239).
//!
//! Chosen by `select::rank` and activated by `encode::find_h264`; then set
//! up the same way whichever it is:
//!
//! - **Asynchronous or synchronous, as the transform says.** A hardware MFT
//!   (NVENC, AMF, Quick Sync) declares `MF_TRANSFORM_ASYNC`, has to be
//!   unlocked with `MF_TRANSFORM_ASYNC_UNLOCK` before anything else, and is
//!   then driven by its events: a frame goes in per `METransformNeedInput`
//!   and a sample comes out per `METransformHaveOutput`, matched up by
//!   `own::mft::AsyncPump` because the cadence loop polls rather than waits.
//!   Microsoft's software MFT is synchronous: `ProcessInput`, then
//!   `ProcessOutput` until it wants more.
//! - **Textures in, where the encoder can read them.** A D3D11-aware
//!   transform (`MF_SA_D3D11_AWARE`, which every hardware encoder is) gets
//!   the session's device through `MFT_MESSAGE_SET_D3D_MANAGER`, and each
//!   frame as the NV12 texture itself, wrapped by `MFCreateDXGISurfaceBuffer`.
//!   Anything else gets NV12 in system memory (`own/win/convert.rs`).
//! - **`ICodecAPI`**: CBR at 8 Mbps, a GOP of 120 (two seconds), low-latency
//!   mode, and **no B-frames**, so output order is presentation order and
//!   the mux needs no composition offsets. A property the encoder refuses is
//!   logged, not fatal; the file says what it got.
//! - **Types**: the output H.264 High at the frame size, 60 fps, 8 Mbps, and
//!   the input NV12, both tagged BT.709 studio range so the stream's VUI
//!   says what the conversion did.
//! - **Keyframes** are read from `MFSampleExtension_CleanPoint`. The SPS and
//!   PPS the file's `moov` needs come from the first keyframe, or from the
//!   output type's `MF_MT_MPEG_SEQUENCE_HEADER` if an encoder does not put
//!   them in-band.

use std::time::{Duration, Instant};

use windows::Win32::Foundation::VARIANT_TRUE;
use windows::Win32::Graphics::Direct3D11::ID3D11Device;
use windows::Win32::Media::MediaFoundation::{
    CODECAPI_AVEncCommonMeanBitRate, CODECAPI_AVEncCommonRateControlMode,
    CODECAPI_AVEncMPVDefaultBPictureCount, CODECAPI_AVEncMPVGOPSize, CODECAPI_AVLowLatencyMode,
    ICodecAPI, IMFActivate, IMFDXGIDeviceManager, IMFMediaEventGenerator, IMFMediaType,
    IMFSample, IMFShutdown, IMFTransform, METransformDrainComplete, METransformHaveOutput,
    METransformNeedInput, MF_E_NO_EVENTS_AVAILABLE, MF_E_NOTACCEPTING, MF_EVENT_FLAG_NO_WAIT,
    MF_MT_AVG_BITRATE, MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE,
    MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE, MF_MT_MPEG_SEQUENCE_HEADER, MF_MT_MPEG2_PROFILE,
    MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SUBTYPE, MF_MT_TRANSFER_FUNCTION,
    MF_MT_VIDEO_NOMINAL_RANGE, MF_MT_VIDEO_PRIMARIES, MF_MT_YUV_MATRIX, MF_SA_D3D11_AWARE,
    MF_TRANSFORM_ASYNC, MF_TRANSFORM_ASYNC_UNLOCK, MFCreateDXGIDeviceManager,
    MFMediaType_Video, MFNominalRange_16_235, MFT_MESSAGE_COMMAND_DRAIN,
    MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, MFT_MESSAGE_NOTIFY_END_OF_STREAM,
    MFT_MESSAGE_NOTIFY_END_STREAMING, MFT_MESSAGE_NOTIFY_START_OF_STREAM,
    MFT_MESSAGE_SET_D3D_MANAGER, MFT_MESSAGE_TYPE, MFVideoFormat_H264, MFVideoFormat_NV12,
    MFVideoInterlace_Progressive, MFVideoPrimaries_BT709, MFVideoTransFunc_709,
    MFVideoTransferMatrix_BT709, eAVEncCommonRateControlMode_CBR, eAVEncH264VProfile_High,
};
use windows::Win32::System::Variant::{
    VARIANT, VARIANT_0, VARIANT_0_0, VARIANT_0_0_0, VT_BOOL, VT_UI4,
};
use windows::core::{GUID, Interface};

use super::encode::{
    self, Encoded, OutputAlloc, VIDEO_BITRATE, GOP_FRAMES, blob_attribute, new_media_type, pack,
    set_guid, set_u32, set_u64, u32_attribute,
};
use crate::mp4::write::extract_parameter_sets;
use crate::recorder::own::mft::{AsyncPump, Event};
use crate::recorder::own::select;
use crate::recorder::own::status::Loaded;

/// Frames an asynchronous encoder may leave waiting for a `NeedInput` before
/// the recording is ended: two seconds. An encoder this far behind has
/// stopped, and every frame waiting holds a texture.
const QUEUE_LIMIT: usize = 120;

/// How long the drain at stop may take before what is out is kept.
const DRAIN_WAIT: Duration = Duration::from_secs(5);

/// How a frame reaches the encoder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputMode {
    /// The NV12 texture itself, through the DXGI device manager.
    Texture,
    /// NV12 bytes in a system-memory buffer.
    Memory,
}

/// One H.264 encoder transform, set up and streaming.
pub struct VideoEncoder {
    transform: IMFTransform,
    activate: IMFActivate,
    /// `Some` for an asynchronous transform, with its bookkeeping.
    events: Option<(IMFMediaEventGenerator, AsyncPump<IMFSample>)>,
    alloc: OutputAlloc,
    /// Kept for the transform's life: it holds the device the textures are on.
    _manager: Option<IMFDXGIDeviceManager>,
    pub input: InputMode,
    pub loaded: Loaded,
    /// `ICodecAPI` properties the encoder refused, for the log.
    pub refused: Vec<String>,
    /// Whether the file's first keyframe has been seen.
    first_keyframe: bool,
    ended: bool,
}

/// A `VT_UI4` variant.
fn variant_u32(value: u32) -> VARIANT {
    VARIANT {
        Anonymous: VARIANT_0 {
            Anonymous: std::mem::ManuallyDrop::new(VARIANT_0_0 {
                vt: VT_UI4,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: VARIANT_0_0_0 { ulVal: value },
            }),
        },
    }
}

/// A `VT_BOOL` variant holding true.
fn variant_true() -> VARIANT {
    VARIANT {
        Anonymous: VARIANT_0 {
            Anonymous: std::mem::ManuallyDrop::new(VARIANT_0_0 {
                vt: VT_BOOL,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: VARIANT_0_0_0 { boolVal: VARIANT_TRUE },
            }),
        },
    }
}

/// Tags `media_type` BT.709, studio range: what `convert.rs` and the video
/// processor produce, and what the stream's VUI should say.
fn tag_bt709(media_type: &IMFMediaType) -> Result<(), String> {
    set_u32(media_type, &MF_MT_VIDEO_PRIMARIES, MFVideoPrimaries_BT709.0 as u32, "the primaries")?;
    set_u32(media_type, &MF_MT_TRANSFER_FUNCTION, MFVideoTransFunc_709.0 as u32, "the transfer")?;
    set_u32(media_type, &MF_MT_YUV_MATRIX, MFVideoTransferMatrix_BT709.0 as u32, "the matrix")?;
    set_u32(media_type, &MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_16_235.0 as u32, "the range")
}

fn video_type(subtype: &GUID, width: u32, height: u32, fps: u32) -> Result<IMFMediaType, String> {
    let media_type = new_media_type()?;
    set_guid(&media_type, &MF_MT_MAJOR_TYPE, &MFMediaType_Video, "the major type")?;
    set_guid(&media_type, &MF_MT_SUBTYPE, subtype, "the subtype")?;
    set_u32(
        &media_type,
        &MF_MT_INTERLACE_MODE,
        MFVideoInterlace_Progressive.0 as u32,
        "the interlace mode",
    )?;
    // Packed pairs in one 64-bit attribute, high half first. Setting them as
    // two 32-bit values fails at SetOutputType with an error that names
    // neither.
    set_u64(&media_type, &MF_MT_FRAME_SIZE, pack(width, height), "the frame size")?;
    set_u64(&media_type, &MF_MT_FRAME_RATE, pack(fps, 1), "the frame rate")?;
    set_u64(&media_type, &MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1), "the pixel aspect ratio")?;
    tag_bt709(&media_type)?;
    Ok(media_type)
}

fn output_type(width: u32, height: u32, fps: u32, high: bool) -> Result<IMFMediaType, String> {
    let media_type = video_type(&MFVideoFormat_H264, width, height, fps)?;
    set_u32(&media_type, &MF_MT_AVG_BITRATE, VIDEO_BITRATE, "the bitrate")?;
    if high {
        set_u32(&media_type, &MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_High.0 as u32, "the profile")?;
    }
    Ok(media_type)
}

fn input_type(width: u32, height: u32, fps: u32) -> Result<IMFMediaType, String> {
    let media_type = video_type(&MFVideoFormat_NV12, width, height, fps)?;
    // Tightly packed, which is what `convert.rs` hands over.
    set_u32(&media_type, &MF_MT_DEFAULT_STRIDE, width, "the stride")?;
    Ok(media_type)
}

fn message(
    transform: &IMFTransform,
    what: &str,
    message: MFT_MESSAGE_TYPE,
    param: usize,
) -> Result<(), String> {
    // SAFETY: `transform` is live; every message this module sends takes 0
    // or, for SET_D3D_MANAGER, a live device manager's pointer.
    unsafe { transform.ProcessMessage(message, param) }.map_err(|e| format!("{what} failed: {e}"))
}

impl VideoEncoder {
    /// Activates `wanted` and sets it up for `width` x `height` at `fps`.
    /// `texture` asks for texture input on `device`, which is only honoured
    /// if the transform is D3D11-aware and takes the device.
    pub fn create(
        wanted: &select::Encoder,
        device: &ID3D11Device,
        width: u32,
        height: u32,
        fps: u32,
        texture: bool,
    ) -> Result<VideoEncoder, String> {
        let activate = encode::find_h264(wanted)?;
        // SAFETY: `activate` is live; the transform is ours until shut down.
        let transform: IMFTransform = unsafe { activate.ActivateObject() }
            .map_err(|e| format!("activating {} failed: {e}", wanted.name))?;
        let mut encoder = VideoEncoder {
            loaded: encode::loaded(&transform, &activate),
            transform,
            activate,
            events: None,
            alloc: OutputAlloc::unset(),
            _manager: None,
            input: InputMode::Memory,
            refused: Vec::new(),
            first_keyframe: false,
            ended: false,
        };
        // From here a failure drops `encoder`, which shuts the transform down.
        encoder.set_up(device, width, height, fps, texture)?;
        Ok(encoder)
    }

    fn set_up(
        &mut self,
        device: &ID3D11Device,
        width: u32,
        height: u32,
        fps: u32,
        texture: bool,
    ) -> Result<(), String> {
        // SAFETY: `transform` is live.
        let attributes = unsafe { self.transform.GetAttributes() }.ok();
        let is_async = attributes
            .as_ref()
            .and_then(|a| u32_attribute(a, &MF_TRANSFORM_ASYNC))
            .is_some_and(|v| v != 0);
        let d3d11_aware = attributes
            .as_ref()
            .and_then(|a| u32_attribute(a, &MF_SA_D3D11_AWARE))
            .is_some_and(|v| v != 0);

        // First, before any other call: an async MFT refuses everything
        // until it is unlocked.
        if is_async {
            let attributes = attributes.as_ref().ok_or("an async MFT with no attributes")?;
            set_u32(attributes, &MF_TRANSFORM_ASYNC_UNLOCK, 1, "MF_TRANSFORM_ASYNC_UNLOCK")?;
        }

        // Before the types: the device decides what input the encoder takes.
        if texture && d3d11_aware {
            let mut token = 0u32;
            let mut manager: Option<IMFDXGIDeviceManager> = None;
            // SAFETY: both out-parameters are live.
            unsafe { MFCreateDXGIDeviceManager(&mut token, &mut manager) }
                .map_err(|e| format!("MFCreateDXGIDeviceManager failed: {e}"))?;
            let manager = manager.ok_or("MFCreateDXGIDeviceManager returned nothing")?;
            // SAFETY: the device is live and the token is the one just issued.
            unsafe { manager.ResetDevice(device, token) }
                .map_err(|e| format!("ResetDevice failed: {e}"))?;
            // The message's parameter is the manager's IUnknown pointer; the
            // transform takes its own reference, and `_manager` keeps ours.
            match message(
                &self.transform,
                "MFT_MESSAGE_SET_D3D_MANAGER",
                MFT_MESSAGE_SET_D3D_MANAGER,
                manager.as_raw() as usize,
            ) {
                Ok(()) => {
                    self.input = InputMode::Texture;
                    self._manager = Some(manager);
                }
                Err(e) => self.refused.push(format!("{e}, so frames go in system memory")),
            }
        } else if texture {
            self.refused.push(
                "the encoder is not D3D11-aware, so frames go in system memory".to_string(),
            );
        }

        self.codec_api();

        // Output first, High profile; without the profile if the encoder
        // will not take it that way.
        let input = input_type(width, height, fps)?;
        let high = output_type(width, height, fps, true)?;
        if let Err(first) = encode::set_types(&self.transform, &input, &high, true) {
            let plain = output_type(width, height, fps, false)?;
            encode::set_types(&self.transform, &input, &plain, true).map_err(|second| {
                format!(
                    "the H.264 encoder takes neither type pair: High profile ({first}); \
                     its default profile ({second})"
                )
            })?;
            self.refused.push(format!("High profile ({first}), so its default profile"));
        }

        self.alloc = OutputAlloc::of(&self.transform, width * height * 3 / 2)?;
        message(&self.transform, "NOTIFY_BEGIN_STREAMING", MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
        message(&self.transform, "NOTIFY_START_OF_STREAM", MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;
        if is_async {
            let events: IMFMediaEventGenerator = self
                .transform
                .cast()
                .map_err(|e| format!("an async MFT with no event generator: {e}"))?;
            self.events = Some((events, AsyncPump::new(QUEUE_LIMIT)));
        }
        Ok(())
    }

    /// The rate control, the GOP, low latency and no B-frames. Each property
    /// the encoder refuses is noted in `refused` rather than failing: an
    /// encoder that will not do low-latency mode still records.
    fn codec_api(&mut self) {
        let api: ICodecAPI = match self.transform.cast() {
            Ok(api) => api,
            Err(e) => {
                self.refused.push(format!("every ICodecAPI property (no ICodecAPI: {e})"));
                return;
            }
        };
        let properties = [
            (CODECAPI_AVEncCommonRateControlMode, variant_u32(eAVEncCommonRateControlMode_CBR.0 as u32), "CBR"),
            (CODECAPI_AVEncCommonMeanBitRate, variant_u32(VIDEO_BITRATE), "8 Mbps"),
            (CODECAPI_AVEncMPVGOPSize, variant_u32(GOP_FRAMES), "GOP 120"),
            (CODECAPI_AVLowLatencyMode, variant_true(), "low latency"),
            (CODECAPI_AVEncMPVDefaultBPictureCount, variant_u32(0), "no B-frames"),
        ];
        for (key, value, what) in properties {
            // SAFETY: `api` is live; `key` and `value` outlive the call, and
            // `value` is a plain VT_UI4 or VT_BOOL that owns nothing.
            if let Err(e) = unsafe { api.SetValue(&key, &value) } {
                self.refused.push(format!("{what} ({e})"));
            }
        }
    }

    /// Whether the transform is asynchronous (a hardware encoder).
    pub fn is_async(&self) -> bool {
        self.events.is_some()
    }

    /// The deepest an asynchronous encoder's frame queue has been.
    pub fn deepest_queue(&self) -> usize {
        self.events.as_ref().map_or(0, |(_, pump)| pump.deepest)
    }

    /// Hands one frame to the encoder, and whatever it has produced to `out`.
    pub fn encode(&mut self, frame: IMFSample, out: &mut Vec<Encoded>) -> Result<(), String> {
        if self.ended {
            return Err("the encoder has been drained".to_string());
        }
        match self.events.as_mut() {
            Some((_, pump)) => {
                if pump.submit(frame).is_err() {
                    return Err(format!(
                        "the hardware encoder has taken no frame for {QUEUE_LIMIT} ticks"
                    ));
                }
                self.poll(out)
            }
            None => self.encode_sync(&frame, out),
        }
    }

    /// Collects what an asynchronous encoder has ready, and feeds it frames
    /// it has asked for. Called on every pass of the cadence loop, so output
    /// does not wait for the next tick. A synchronous encoder has nothing
    /// pending between frames.
    pub fn poll(&mut self, out: &mut Vec<Encoded>) -> Result<(), String> {
        let Some((events, _)) = self.events.as_ref() else {
            return Ok(());
        };
        let events = events.clone();
        loop {
            // SAFETY: `events` is live; NO_WAIT returns at once.
            let event = match unsafe { events.GetEvent(MF_EVENT_FLAG_NO_WAIT) } {
                Ok(event) => event,
                Err(e) if e.code() == MF_E_NO_EVENTS_AVAILABLE => break,
                Err(e) => return Err(format!("the encoder's GetEvent failed: {e}")),
            };
            // SAFETY: `event` is live.
            let kind = unsafe { event.GetType() }.map_err(|e| format!("GetType failed: {e}"))?;
            // SAFETY: `event` is live.
            if let Ok(status) = unsafe { event.GetStatus() }
                && status.is_err()
            {
                return Err(format!("the encoder reported an error event ({kind}): {status}"));
            }
            let kind = match kind {
                k if k == METransformNeedInput.0 as u32 => Event::NeedInput,
                k if k == METransformHaveOutput.0 as u32 => Event::HaveOutput,
                k if k == METransformDrainComplete.0 as u32 => Event::DrainComplete,
                _ => Event::Other,
            };
            if let Some((_, pump)) = self.events.as_mut() {
                pump.event(kind);
            }
            self.act(out)?;
        }
        self.act(out)
    }

    /// Spends every credit on a waiting frame, and makes every
    /// `ProcessOutput` owed.
    fn act(&mut self, out: &mut Vec<Encoded>) -> Result<(), String> {
        loop {
            let Some((_, pump)) = self.events.as_mut() else {
                return Ok(());
            };
            if let Some(frame) = pump.next_input() {
                // SAFETY: the transform asked for input; stream 0, no flags.
                unsafe { self.transform.ProcessInput(0, &frame, 0) }
                    .map_err(|e| format!("ProcessInput failed: {e}"))?;
                continue;
            }
            if pump.next_output() {
                if let encode::Output::Sample(sample) =
                    encode::process_output(&self.transform, self.alloc)?
                {
                    self.push(sample, out);
                }
                continue;
            }
            return Ok(());
        }
    }

    fn encode_sync(&mut self, frame: &IMFSample, out: &mut Vec<Encoded>) -> Result<(), String> {
        for _ in 0..2 {
            // SAFETY: `frame` is live; stream 0, no flags.
            match unsafe { self.transform.ProcessInput(0, frame, 0) } {
                Ok(()) => return self.drain_sync(out),
                // Full: take what it has, then it will accept this one.
                Err(e) if e.code() == MF_E_NOTACCEPTING => self.drain_sync(out)?,
                Err(e) => return Err(format!("ProcessInput failed: {e}")),
            }
        }
        Err("the encoder would not accept a frame even after its output was taken".to_string())
    }

    fn drain_sync(&mut self, out: &mut Vec<Encoded>) -> Result<(), String> {
        let mut produced = Vec::new();
        encode::drain_outputs(&self.transform, self.alloc, &mut produced)?;
        for sample in produced {
            self.push(sample, out);
        }
        Ok(())
    }

    /// Hands a sample on, putting the SPS and PPS in front of the file's
    /// first keyframe if the encoder did not.
    fn push(&mut self, mut sample: Encoded, out: &mut Vec<Encoded>) {
        if sample.keyframe && !self.first_keyframe {
            self.first_keyframe = true;
            if extract_parameter_sets(&sample.bytes).is_none()
                && let Some(header) = self.sequence_header()
            {
                let mut bytes = header;
                bytes.extend_from_slice(&sample.bytes);
                sample.bytes = bytes;
            }
        }
        out.push(sample);
    }

    /// The SPS and PPS from the output type, Annex B, if the encoder
    /// publishes them there.
    fn sequence_header(&self) -> Option<Vec<u8>> {
        // SAFETY: `transform` is live; stream 0.
        let current = unsafe { self.transform.GetOutputCurrentType(0) }.ok()?;
        blob_attribute(&current, &MF_MT_MPEG_SEQUENCE_HEADER)
    }

    /// Takes every frame still inside the encoder: the last frames of a
    /// recording. Waits up to [`DRAIN_WAIT`] for an asynchronous encoder,
    /// then keeps what came out.
    pub fn drain(&mut self, out: &mut Vec<Encoded>) -> Result<(), String> {
        if self.ended {
            return Ok(());
        }
        self.ended = true;
        let deadline = Instant::now() + DRAIN_WAIT;
        if self.events.is_some() {
            // Whatever is queued goes in first, as the encoder asks for it.
            while self.events.as_ref().is_some_and(|(_, pump)| !pump.ready_to_drain()) {
                self.poll(out)?;
                if Instant::now() >= deadline {
                    return Err("the hardware encoder stopped asking for frames at stop".into());
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        message(&self.transform, "NOTIFY_END_OF_STREAM", MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0)?;
        message(&self.transform, "COMMAND_DRAIN", MFT_MESSAGE_COMMAND_DRAIN, 0)?;
        match self.events.as_mut() {
            None => self.drain_sync(out)?,
            Some((_, pump)) => {
                pump.start_drain();
                while !self.events.as_ref().is_some_and(|(_, pump)| pump.drained()) {
                    self.poll(out)?;
                    if Instant::now() >= deadline {
                        return Err(format!(
                            "the hardware encoder did not finish draining within {} s",
                            DRAIN_WAIT.as_secs()
                        ));
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
        }
        message(&self.transform, "NOTIFY_END_STREAMING", MFT_MESSAGE_NOTIFY_END_STREAMING, 0)
    }
}

impl Drop for VideoEncoder {
    /// Shuts the transform down, which a hardware encoder needs to release
    /// its session (an NVENC session is a limited resource), then the
    /// activation object.
    fn drop(&mut self) {
        if let Ok(shutdown) = self.transform.cast::<IMFShutdown>() {
            // SAFETY: `shutdown` is live; nothing uses the transform after.
            let _ = unsafe { shutdown.Shutdown() };
        }
        // SAFETY: `activate` made the transform; nothing uses it after this.
        let _ = unsafe { self.activate.ShutdownObject() };
    }
}
