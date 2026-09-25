//! Process loopback: an `IAudioClient` bound to one process tree. Ported
//! from `spikes/p0c-audio`, which proved it against League on the box (#7,
//! DEVELOPMENT.md §16). The game's tree since #237, and an application's
//! (Discord's, rooted by `root::application_root`) since #238.
//!
//! **Nothing here touches the game process.** Process loopback is the audio
//! engine handing over the mix of one process tree's streams; the target is
//! named by PID and never opened. It is the same API the libobs fork's
//! process-output source calls, and no injection is involved (§1.1).

use std::mem::ManuallyDrop;

use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
use windows::Win32::Media::Audio::{
    AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_LOOPBACK,
    AUDIOCLIENT_ACTIVATION_PARAMS, AUDIOCLIENT_ACTIVATION_PARAMS_0,
    AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK, AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS,
    ActivateAudioInterfaceAsync, IActivateAudioInterfaceAsyncOperation,
    IActivateAudioInterfaceCompletionHandler, IActivateAudioInterfaceCompletionHandler_Impl,
    IAudioCaptureClient, IAudioClient, PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
    VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
};
use windows::Win32::System::Com::BLOB;
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForSingleObject};
use windows::Win32::System::Variant::VT_BLOB;
use windows::core::{Interface, PCWSTR, Ref, implement};

use super::super::process::OwnedHandle;
use super::{Capture, Raw, float_format, read_packet};

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
    /// stereo float, event-driven. **Process loopback has no
    /// `GetMixFormat`**: there is no device to ask, so the format is asserted
    /// and the engine converts into it.
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
}

impl Capture for Loopback {
    /// Waits up to `ms` for the engine to signal a packet. A timeout is not
    /// an error: a process-loopback stream is not guaranteed to be fed while
    /// its target is silent.
    fn wait(&self, ms: u32) -> bool {
        // SAFETY: `ready` is a live event.
        let wait = unsafe { WaitForSingleObject(self.ready.0, ms) };
        wait == WAIT_OBJECT_0
    }

    fn next(&self) -> Result<Option<Raw>, String> {
        read_packet(&self.capture)
    }

    fn stop(&self) {
        // SAFETY: `client` is live; stopping has no other precondition.
        let _ = unsafe { self.client.Stop() };
    }
}
