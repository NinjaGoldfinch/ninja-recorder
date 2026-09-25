//! Endpoint capture: the microphone, and the desktop by render loopback.
//! Ported from `spikes/p0c-video/src/win/audio.rs`, which captured both on
//! the box for #8, and whose loopback figures are §16's drift row.
//!
//! Two differences from the spike, both from the plan (#238):
//!
//! - **No mix format, no resampler.** The spike asked `GetMixFormat` and
//!   refused a device that was not at 44.1 or 48 kHz. Here the client is
//!   initialised with `AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM` and
//!   `AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY` for 48 kHz stereo float, the
//!   format every source uses, so the audio engine converts whatever the
//!   device runs at and this crate writes no resampler. The same format is
//!   what process loopback asserts (`loopback.rs`).
//! - **The default microphone is the communications one.** `eCapture` with
//!   `eCommunications`, as the picker's "Windows default" entry resolves it
//!   (`recorder/devices.rs`) and as libobs does for an input whose id is
//!   "default", so the two backends open the same microphone. A configured
//!   device is opened by the id the picker stored, through `GetDevice`.
//!
//! **The desktop needs a keep-alive.** A render endpoint in loopback
//! delivers no packets at all while nothing is playing, so the thread also
//! plays a stream of silence to the same endpoint, which keeps the engine
//! running and the packets coming at the device's own rate. A capture
//! endpoint needs none: it delivers continuously.
//!
//! Both are polled every few milliseconds, as the spike polled them, rather
//! than event-driven: the keep-alive has to be topped up on the same cadence
//! anyway.

use std::time::Duration;

use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Media::Audio::{
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
    AUDCLNT_STREAMFLAGS_LOOPBACK, AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, IAudioCaptureClient,
    IAudioClient, IAudioRenderClient, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator,
    eCapture, eCommunications, eConsole, eRender,
};
use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance, CoTaskMemFree, STGM_READ};
use windows::core::{PCWSTR, PWSTR};

use super::{Capture, Raw, float_format, read_packet};

/// 100 ms of endpoint buffer, polled every 5 ms: generous enough that a slow
/// poll never overflows it, which would be a discontinuity we caused.
const BUFFER_HNS: i64 = 1_000_000;
const POLL: Duration = Duration::from_millis(5);

/// The engine converts to the format asked for, at its default quality.
const CONVERT: u32 = AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;

/// Which endpoint.
pub enum Kind {
    /// An input: `None` for Windows' default communications device, or the
    /// endpoint id the microphone picker stored.
    Microphone(Option<String>),
    /// The default output device, in loopback.
    Desktop,
}

/// The silent stream that keeps a loopback endpoint delivering.
struct KeepAlive {
    client: IAudioClient,
    render: IAudioRenderClient,
    /// The render buffer's size in frames.
    size: u32,
}

impl KeepAlive {
    /// Fills whatever the engine has played since the last call with
    /// silence. Failure is ignored: the worst it costs is a quiet stretch
    /// with no packets, which the mixer covers anyway.
    fn top_up(&self) {
        // SAFETY: the client is initialised and started.
        let Ok(padding) = (unsafe { self.client.GetCurrentPadding() }) else {
            return;
        };
        let free = self.size.saturating_sub(padding);
        // SAFETY: `free` frames are available by the padding just read.
        if free > 0 && unsafe { self.render.GetBuffer(free) }.is_ok() {
            // SAFETY: releases exactly the frames just obtained, as silence,
            // so their contents are never read.
            let _ = unsafe { self.render.ReleaseBuffer(free, AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) };
        }
    }
}

/// An initialised, polled endpoint capture.
pub struct Endpoint {
    client: IAudioClient,
    capture: IAudioCaptureClient,
    keep_alive: Option<KeepAlive>,
    /// What was opened, for the log: which device, by name and id, and how
    /// it was chosen.
    description: String,
}

/// Copies a COM-allocated wide string out, then frees it.
///
/// # Safety
///
/// `ptr` must be null, or a `CoTaskMemAlloc`-ed, null-terminated wide string
/// whose ownership passes to this function. It is freed here.
unsafe fn take_pwstr(ptr: PWSTR) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: non-null, and null-terminated by the caller's contract.
    let out = unsafe { PCWSTR(ptr.0).to_string() }.ok();
    // SAFETY: allocated with CoTaskMemAlloc and ours, per the contract; the
    // string is already copied out.
    unsafe { CoTaskMemFree(Some(ptr.0 as *const core::ffi::c_void)) };
    out
}

/// The device's endpoint id and friendly name, for the log.
fn identify(device: &IMMDevice) -> (String, String) {
    // SAFETY: `device` is live; `GetId` hands over a CoTaskMemAlloc-ed
    // string, which `take_pwstr` frees.
    let id = unsafe { device.GetId() }
        .ok()
        .and_then(|id| unsafe { take_pwstr(id) })
        .unwrap_or_else(|| "(no id)".to_string());
    // SAFETY: `device` is live, and so is the store it opens. The
    // `PROPVARIANT` is ours and windows-rs clears it on drop; the string
    // `PropVariantToStringAlloc` returns is CoTaskMemAlloc-ed and ours.
    let name = unsafe { device.OpenPropertyStore(STGM_READ) }
        .ok()
        .and_then(|store| unsafe { store.GetValue(&PKEY_Device_FriendlyName) }.ok())
        .and_then(|value| unsafe { PropVariantToStringAlloc(&value) }.ok())
        .and_then(|name| unsafe { take_pwstr(name) })
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "(unnamed)".to_string());
    (id, name)
}

/// Opens the device for `kind`, and says how it was chosen.
fn device(kind: &Kind) -> Result<(IMMDevice, &'static str), String> {
    // SAFETY: COM is initialised (MTA) on this thread, and the CLSID and the
    // interface are a matching pair.
    let enumerator: IMMDeviceEnumerator =
        unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
            .map_err(|e| format!("could not create the device enumerator: {e}"))?;
    match kind {
        Kind::Microphone(None) => {
            // SAFETY: `enumerator` is live.
            let device = unsafe { enumerator.GetDefaultAudioEndpoint(eCapture, eCommunications) }
                .map_err(|e| format!("there is no default microphone: {e}"))?;
            Ok((device, "the default communications microphone"))
        }
        Kind::Microphone(Some(id)) => {
            let wide: Vec<u16> = id.encode_utf16().chain(std::iter::once(0)).collect();
            // SAFETY: `enumerator` is live, and `wide` is a null-terminated
            // wide string that outlives the call.
            let device = unsafe { enumerator.GetDevice(PCWSTR(wide.as_ptr())) }
                .map_err(|e| format!("the configured microphone {id} is not there: {e}"))?;
            Ok((device, "the configured microphone"))
        }
        Kind::Desktop => {
            // SAFETY: `enumerator` is live.
            let device = unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) }
                .map_err(|e| format!("there is no default output device: {e}"))?;
            Ok((device, "the default output, in loopback"))
        }
    }
}

/// An `IAudioClient` on `device`, initialised shared for 48 kHz stereo float
/// with the engine converting, plus `flags`.
fn open_client(device: &IMMDevice, flags: u32, what: &str) -> Result<IAudioClient, String> {
    // SAFETY: `device` is live; no activation parameters.
    let client: IAudioClient = unsafe { device.Activate(CLSCTX_ALL, None) }
        .map_err(|e| format!("could not activate {what}: {e}"))?;
    let format = float_format();
    // SAFETY: `client` is live and `format` outlives the call.
    unsafe { client.Initialize(AUDCLNT_SHAREMODE_SHARED, CONVERT | flags, BUFFER_HNS, 0, &format, None) }
        .map_err(|e| format!("IAudioClient::Initialize ({what}) failed: {e}"))?;
    Ok(client)
}

impl Endpoint {
    /// Opens `kind`'s endpoint for 48 kHz stereo float capture, with a
    /// keep-alive stream for the desktop.
    pub fn open(kind: Kind) -> Result<Endpoint, String> {
        let (device, how) = device(&kind)?;
        let (id, name) = identify(&device);
        let loopback = matches!(kind, Kind::Desktop);
        let what = if loopback { "the output device in loopback" } else { "the microphone" };
        let flags = if loopback { AUDCLNT_STREAMFLAGS_LOOPBACK } else { 0 };
        let client = open_client(&device, flags, what)?;
        // SAFETY: `client` is initialised, which GetService requires.
        let capture: IAudioCaptureClient = unsafe { client.GetService() }
            .map_err(|e| format!("could not get IAudioCaptureClient: {e}"))?;

        let keep_alive = if loopback {
            let keep = open_client(&device, 0, "the keep-alive stream")?;
            // SAFETY: initialised.
            let size = unsafe { keep.GetBufferSize() }
                .map_err(|e| format!("GetBufferSize (keep-alive) failed: {e}"))?;
            // SAFETY: initialised, which GetService requires.
            let render: IAudioRenderClient = unsafe { keep.GetService() }
                .map_err(|e| format!("could not get IAudioRenderClient: {e}"))?;
            Some(KeepAlive { client: keep, render, size })
        } else {
            None
        };
        let description = format!(
            "{how}, {name} ({id}){}",
            if keep_alive.is_some() { ", kept running by a silent stream" } else { "" }
        );
        Ok(Endpoint { client, capture, keep_alive, description })
    }

    pub fn describe(&self) -> &str {
        &self.description
    }

    /// Starts the keep-alive, full of silence, and then the capture.
    pub fn start(&self) -> Result<(), String> {
        if let Some(keep) = &self.keep_alive {
            keep.top_up();
            // SAFETY: the keep-alive client is initialised.
            unsafe { keep.client.Start() }
                .map_err(|e| format!("the keep-alive stream did not start: {e}"))?;
        }
        // SAFETY: the client is initialised.
        unsafe { self.client.Start() }.map_err(|e| format!("IAudioClient::Start failed: {e}"))
    }
}

impl Capture for Endpoint {
    /// Tops the keep-alive up and sleeps one poll. Always `true`: the caller
    /// drains whatever has arrived, which may be nothing.
    fn wait(&self, _ms: u32) -> bool {
        if let Some(keep) = &self.keep_alive {
            keep.top_up();
        }
        std::thread::sleep(POLL);
        true
    }

    fn next(&self) -> Result<Option<Raw>, String> {
        read_packet(&self.capture)
    }

    fn stop(&self) {
        // SAFETY: the clients are live; stopping has no other precondition.
        let _ = unsafe { self.client.Stop() };
        if let Some(keep) = &self.keep_alive {
            // SAFETY: as above.
            let _ = unsafe { keep.client.Stop() };
        }
    }
}
