//! Process loopback: an `IAudioClient` bound to one process tree. Ported
//! from `spikes/p0c-audio`, which proved it against League on the box (#7,
//! DEVELOPMENT.md §16).
//!
//! **Nothing here touches the game process.** Process loopback is the audio
//! engine handing over the mix of one process tree's streams; the target is
//! named by PID and never opened. It is the same API the libobs fork's
//! process-output source calls, and no injection is involved (§1.1).

use std::mem::ManuallyDrop;

use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
use windows::Win32::Media::Audio::{
    AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY, AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED,
    AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_LOOPBACK,
    AUDIOCLIENT_ACTIVATION_PARAMS, AUDIOCLIENT_ACTIVATION_PARAMS_0,
    AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK, AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS,
    ActivateAudioInterfaceAsync, IActivateAudioInterfaceAsyncOperation,
    IActivateAudioInterfaceCompletionHandler, IActivateAudioInterfaceCompletionHandler_Impl,
    IAudioCaptureClient, IAudioClient, PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
    VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK, WAVEFORMATEX,
};
use windows::Win32::System::Com::BLOB;
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForSingleObject};
use windows::Win32::System::Variant::VT_BLOB;
use windows::core::{Interface, PCWSTR, Ref, implement};

use super::super::device;
use super::super::process::OwnedHandle;
use crate::recorder::own::pcm::{self, SampleFormat};

/// `WAVE_FORMAT_IEEE_FLOAT`, spelled out rather than pulling in the kernel
/// streaming feature for one documented constant.
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;

/// The format asked for. **Process loopback has no `GetMixFormat`**: there
/// is no device to ask, so the format is asserted and the engine converts
/// into it. 48 kHz stereo float is what the mix graph runs at natively, and
/// 48 kHz is a rate the AAC encoder takes.
pub const SAMPLE_RATE: u32 = 48_000;
const CHANNELS: u16 = 2;
const BITS: u16 = 32;

/// How long the activation may take before it counts as hung.
const ACTIVATION_TIMEOUT_MS: u32 = 5_000;

/// The completion handler `ActivateAudioInterfaceAsync` calls. It only
/// signals: the result is read from the operation afterwards, on the calling
/// thread, where the error can be reported with what was being attempted.
#[implement(IActivateAudioInterfaceCompletionHandler)]
struct ActivationHandler(HANDLE);

impl IActivateAudioInterfaceCompletionHandler_Impl for ActivationHandler_Impl {
    fn ActivateCompleted(
        &self,
        _operation: Ref<IActivateAudioInterfaceAsyncOperation>,
    ) -> windows::core::Result<()> {
        // SAFETY: the event is owned by `activate`, which waits on it before
        // dropping it, and leaks it rather than close it if that wait times
        // out, so a late completion never signals a handle value that has
        // since been reused.
        unsafe { SetEvent(self.0) }
    }
}

fn new_event() -> Result<OwnedHandle, String> {
    // SAFETY: no security attributes and no name; an auto-reset event,
    // initially unsignalled. The handle is owned by the returned value.
    let handle = unsafe { CreateEventW(None, false, false, PCWSTR::null()) }
        .map_err(|e| format!("CreateEventW failed: {e}"))?;
    Ok(OwnedHandle(handle))
}

/// Activates an `IAudioClient` for `pid` **and its descendants**
/// (`PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE`).
fn activate(pid: u32) -> Result<IAudioClient, String> {
    let activated = new_event()?;

    let mut params = AUDIOCLIENT_ACTIVATION_PARAMS {
        ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
        Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
            ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                TargetProcessId: pid,
                // **The tree, not the process.** A modern game is several
                // processes and the one that plays audio is often not the one
                // that owns the window. There is no single-process mode to ask
                // for anyway: include and exclude are the only values.
                ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
            },
        },
    };

    // The activation parameters travel inside a PROPVARIANT as a raw blob,
    // built by hand because there is no constructor for VT_BLOB.
    //
    // **ManuallyDrop, because this PROPVARIANT does have a Drop.** windows-rs
    // adds one in `extensions/Win32/System/StructuredStorage.rs` that calls
    // `PropVariantClear`, which hands `pBlobData` to `CoTaskMemFree`. The
    // blob here points at `params` on the stack, so letting it drop freed a
    // stack address on the way out of `activate` and the process died with
    // 0xC0000374 (STATUS_HEAP_CORRUPTION) before capture began (#7, #218).
    // Nothing here was allocated, so there is nothing to clear.
    let mut variant = ManuallyDrop::new(PROPVARIANT::default());
    // SAFETY: writing the active arm of a zeroed union, which is what the
    // callee reads given `vt = VT_BLOB`. `params` outlives the call.
    unsafe {
        let inner = &mut variant.Anonymous.Anonymous;
        inner.vt = VT_BLOB;
        inner.Anonymous.blob = BLOB {
            cbSize: size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32,
            pBlobData: (&raw mut params).cast::<u8>(),
        };
    }

    let handler: IActivateAudioInterfaceCompletionHandler = ActivationHandler(activated.0).into();

    // SAFETY: the device path is a static string, the IID is a static,
    // `variant` and the blob it points at live until after the wait below,
    // and the handler is a live COM object.
    let operation = unsafe {
        ActivateAudioInterfaceAsync(
            VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
            &IAudioClient::IID,
            Some(&raw const *variant),
            &handler,
        )
    }
    .map_err(|e| format!("ActivateAudioInterfaceAsync failed: {e}"))?;

    // SAFETY: `activated` is a live event that only the handler signals.
    let wait = unsafe { WaitForSingleObject(activated.0, ACTIVATION_TIMEOUT_MS) };
    if wait != WAIT_OBJECT_0 {
        // The handler still holds this handle and may yet fire.
        std::mem::forget(activated);
        return Err(format!(
            "the process-loopback activation for PID {pid} never completed ({} s)",
            ACTIVATION_TIMEOUT_MS / 1000
        ));
    }

    let mut result = windows::core::HRESULT(0);
    let mut interface: Option<windows::core::IUnknown> = None;
    // SAFETY: the operation has completed (the handler ran), and both
    // out-parameters are live.
    unsafe { operation.GetActivateResult(&mut result, &mut interface) }
        .map_err(|e| format!("GetActivateResult failed: {e}"))?;
    result
        .ok()
        .map_err(|e| format!("process-loopback activation for PID {pid} was refused: {e}"))?;
    interface
        .ok_or_else(|| "the activation succeeded and returned no interface".to_string())?
        .cast::<IAudioClient>()
        .map_err(|e| format!("the activated object is not an IAudioClient: {e}"))
}

fn float_format() -> WAVEFORMATEX {
    let block_align = CHANNELS * BITS / 8;
    WAVEFORMATEX {
        wFormatTag: WAVE_FORMAT_IEEE_FLOAT,
        nChannels: CHANNELS,
        nSamplesPerSec: SAMPLE_RATE,
        nAvgBytesPerSec: SAMPLE_RATE * u32::from(block_align),
        nBlockAlign: block_align,
        wBitsPerSample: BITS,
        cbSize: 0,
    }
}

/// One packet as `GetBuffer` handed it over.
pub struct Raw {
    pub frames: u32,
    pub silent: bool,
    pub discontinuity: bool,
    /// `pu64QPCPosition`, as the engine gave it: 100 ns units if it is the
    /// performance counter, and the thing `clock::check_stamp` judges.
    pub qpc: u64,
    /// `pu64DevicePosition`, in frames. Logged for the first packet only,
    /// alongside the QPC stamp, so a box run shows both.
    pub device_position: u64,
    /// The performance counter read just after `GetBuffer` returned.
    pub arrival: i64,
    /// Stereo i16, interleaved; zeros for a silent packet.
    pub pcm: Vec<i16>,
}

/// An initialised, event-driven process-loopback capture.
pub struct Loopback {
    client: IAudioClient,
    capture: IAudioCaptureClient,
    /// Signalled by the engine when a packet is ready. Declared after the
    /// clients so it outlives them; `stop` is called before either drops.
    ready: OwnedHandle,
}

impl Loopback {
    /// Activates the tree rooted at `pid` and initialises it for 48 kHz
    /// stereo float, event-driven.
    pub fn open(pid: u32) -> Result<Loopback, String> {
        let client = activate(pid)?;
        let format = float_format();
        // `AUDCLNT_STREAMFLAGS_LOOPBACK` is required even though the
        // activation already said "process loopback": it is what makes the
        // client a capture client on a render stream.
        // SAFETY: `client` is live and `format` outlives the call.
        unsafe {
            client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                // Buffer duration and periodicity must both be 0 for a
                // process-loopback client; anything else is E_INVALIDARG.
                0,
                0,
                &format,
                None,
            )
        }
        .map_err(|e| format!("IAudioClient::Initialize (process loopback) failed: {e}"))?;

        let ready = new_event()?;
        // SAFETY: `ready` is live and outlives the client's use of it: the
        // client is stopped before `ready` drops.
        unsafe { client.SetEventHandle(ready.0) }
            .map_err(|e| format!("SetEventHandle failed: {e}"))?;
        // SAFETY: `client` is initialised, which GetService requires.
        let capture: IAudioCaptureClient = unsafe { client.GetService() }
            .map_err(|e| format!("could not get IAudioCaptureClient: {e}"))?;
        Ok(Loopback { client, capture, ready })
    }

    pub fn start(&self) -> Result<(), String> {
        // SAFETY: `client` is initialised with an event handle set.
        unsafe { self.client.Start() }.map_err(|e| format!("IAudioClient::Start failed: {e}"))
    }

    pub fn stop(&self) {
        // SAFETY: `client` is live; stopping has no other precondition.
        let _ = unsafe { self.client.Stop() };
    }

    /// Waits up to `ms` for the engine to signal a packet. A timeout is not
    /// an error: a process-loopback stream is not guaranteed to be fed while
    /// its target is silent.
    pub fn wait(&self, ms: u32) -> bool {
        // SAFETY: `ready` is a live event.
        let wait = unsafe { WaitForSingleObject(self.ready.0, ms) };
        wait == WAIT_OBJECT_0
    }

    /// The next packet, or `None` when the engine has none waiting. One
    /// event can cover several packets, so the caller drains until `None`:
    /// leaving one behind makes the next `GetBuffer` return it late.
    pub fn next(&self) -> Result<Option<Raw>, String> {
        // SAFETY: the client is started and live.
        let available = unsafe { self.capture.GetNextPacketSize() }
            .map_err(|e| format!("GetNextPacketSize failed: {e}"))?;
        if available == 0 {
            return Ok(None);
        }
        let mut data: *mut u8 = std::ptr::null_mut();
        let mut frames = 0u32;
        let mut flags = 0u32;
        let mut device_position = 0u64;
        let mut qpc = 0u64;
        // **Both positions are asked for.** The spike asked for neither; the
        // QPC one is what puts this source on the video's clock, if it is
        // real, and `clock::Stamper` decides whether it is.
        // SAFETY: every out-parameter is live; the buffer is released below
        // before the next GetBuffer.
        unsafe {
            self.capture.GetBuffer(
                &mut data,
                &mut frames,
                &mut flags,
                Some(&mut device_position),
                Some(&mut qpc),
            )
        }
        .map_err(|e| format!("GetBuffer failed: {e}"))?;
        let arrival = device::qpc_hns();

        let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0;
        let pcm = if silent || data.is_null() {
            // A silent packet's buffer contents are undefined, not zero.
            vec![0i16; frames as usize * 2]
        } else {
            let bytes = frames as usize * usize::from(CHANNELS) * SampleFormat::F32.bytes();
            // SAFETY: the engine owns `bytes` bytes at `data` until
            // ReleaseBuffer, in the format Initialize accepted.
            let raw = unsafe { std::slice::from_raw_parts(data, bytes) };
            pcm::to_stereo_i16(raw, SampleFormat::F32, CHANNELS, frames)
        };
        // SAFETY: releases exactly the packet GetBuffer handed out.
        unsafe { self.capture.ReleaseBuffer(frames) }
            .map_err(|e| format!("ReleaseBuffer failed: {e}"))?;

        Ok(Some(Raw {
            frames,
            silent,
            discontinuity: flags & AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY.0 as u32 != 0,
            qpc,
            device_position,
            arrival,
            pcm,
        }))
    }
}
