//! One WASAPI endpoint, on its own thread, stamped with QPC.
//!
//! The thread does nothing but capture: it converts each packet to stereo
//! i16 and sends it, with the QPC time WASAPI gave its first frame, to the
//! main loop, which owns the sink writer and the [`crate::clock::Aligner`].
//! One writer thread means no question about the sink writer's own locking.
//!
//! **Loopback needs a keep-alive.** A render endpoint in loopback delivers no
//! packets at all while nothing is playing, and a silent stretch of the game
//! would then look like a hole in the audio clock. So in `system` mode the
//! thread also plays a stream of silence to the same endpoint, which keeps the
//! engine running and the packets coming at the device's own rate. The mic
//! needs none: a capture endpoint delivers continuously.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Sender, SyncSender, sync_channel};
use std::thread::JoinHandle;
use std::time::Duration;

use windows::Win32::Media::Audio::{
    AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY, AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED,
    AUDCLNT_STREAMFLAGS_LOOPBACK, IAudioCaptureClient, IAudioClient, IAudioRenderClient, IMMDevice,
    IMMDeviceEnumerator, MMDeviceEnumerator, WAVEFORMATEX, WAVEFORMATEXTENSIBLE, eCapture,
    eConsole, eRender,
};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
    CoUninitialize,
};
use windows::core::GUID;

use crate::AudioSource;
use crate::pcm::{self, SampleFormat};

/// 100 ms of endpoint buffer, polled every 5 ms: generous enough that a slow
/// poll never overflows it, which would be a discontinuity we caused.
const BUFFER_HNS: i64 = 1_000_000;
const POLL: Duration = Duration::from_millis(5);

const WAVE_FORMAT_PCM: u16 = 1;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;
/// `KSDATAFORMAT_SUBTYPE_PCM` and `_IEEE_FLOAT`, spelled out rather than
/// pulling in the kernel-streaming feature for two GUIDs.
const SUBTYPE_PCM: GUID = GUID::from_u128(0x00000001_0000_0010_8000_00aa00389b71);
const SUBTYPE_FLOAT: GUID = GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71);

pub struct Packet {
    /// QPC time of the packet's first frame, in 100 ns units.
    pub qpc_hns: i64,
    pub frames: u32,
    pub discontinuity: bool,
    /// Stereo i16, interleaved; zeros for a packet the engine marked silent.
    pub pcm: Vec<i16>,
}

pub struct Info {
    pub source: &'static str,
    pub device_id: String,
    pub rate: u32,
    pub channels: u16,
    pub format: SampleFormat,
    pub keep_alive: bool,
}

pub struct Handle {
    pub info: Info,
    stop: Arc<AtomicBool>,
    thread: JoinHandle<Result<(), String>>,
}

impl Handle {
    pub fn stop(self) -> Result<(), String> {
        self.stop.store(true, Ordering::Release);
        self.thread
            .join()
            .map_err(|_| "the audio thread panicked".to_string())?
    }
}

/// Open the endpoint and start capturing. Returns once the format is known,
/// because the sink writer's audio stream is built from it.
pub fn start(source: AudioSource, packets: Sender<Packet>) -> Result<Handle, String> {
    let (ready_tx, ready_rx) = sync_channel::<Result<Info, String>>(1);
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = stop.clone();
    let thread = std::thread::Builder::new()
        .name("p0c-audio-capture".into())
        .spawn(move || thread_main(source, packets, ready_tx, thread_stop))
        .map_err(|e| format!("could not start the audio thread: {e}"))?;
    let info = ready_rx
        .recv()
        .map_err(|_| "the audio thread exited before reporting its format".to_string())??;
    Ok(Handle { info, stop, thread })
}

fn thread_main(
    source: AudioSource,
    packets: Sender<Packet>,
    ready: SyncSender<Result<Info, String>>,
    stop: Arc<AtomicBool>,
) -> Result<(), String> {
    // SAFETY: once, on this thread, before any COM use; paired below.
    if let Err(e) = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.ok() {
        let _ = ready.send(Err(format!("CoInitializeEx (audio thread) failed: {e}")));
        return Ok(());
    }
    let result = capture(source, &packets, &ready, &stop);
    // SAFETY: pairs the CoInitializeEx above; every COM object is dropped.
    unsafe { CoUninitialize() };
    result
}

struct Mix {
    format: SampleFormat,
    rate: u32,
    channels: u16,
}

fn describe(pointer: *const WAVEFORMATEX) -> Result<Mix, String> {
    // SAFETY: GetMixFormat returned a valid WAVEFORMATEX. The struct is
    // packed, so it is read unaligned.
    let base = unsafe { std::ptr::read_unaligned(pointer) };
    let tag = base.wFormatTag;
    let bits = base.wBitsPerSample;
    let (tag, valid) = if tag == WAVE_FORMAT_EXTENSIBLE {
        // SAFETY: the tag says the allocation is a WAVEFORMATEXTENSIBLE.
        let ext = unsafe { std::ptr::read_unaligned(pointer.cast::<WAVEFORMATEXTENSIBLE>()) };
        let sub = ext.SubFormat;
        let kind = if sub == SUBTYPE_FLOAT {
            WAVE_FORMAT_IEEE_FLOAT
        } else if sub == SUBTYPE_PCM {
            WAVE_FORMAT_PCM
        } else {
            0
        };
        (kind, bits)
    } else {
        (tag, bits)
    };
    let format = match (tag, valid) {
        (WAVE_FORMAT_IEEE_FLOAT, 32) => SampleFormat::F32,
        (WAVE_FORMAT_PCM, 16) => SampleFormat::I16,
        (WAVE_FORMAT_PCM, 24) => SampleFormat::I24,
        (WAVE_FORMAT_PCM, 32) => SampleFormat::I32,
        _ => {
            return Err(format!(
                "unsupported mix format: tag {tag:#x}, {valid} bits"
            ));
        }
    };
    Ok(Mix {
        format,
        rate: base.nSamplesPerSec,
        channels: base.nChannels,
    })
}

fn device_id(device: &IMMDevice) -> String {
    // SAFETY: `device` is live; the string is ours to free.
    let Ok(id) = (unsafe { device.GetId() }) else {
        return "(unknown)".to_string();
    };
    // SAFETY: GetId returned a NUL-terminated wide string.
    let text = unsafe { id.to_string() }.unwrap_or_default();
    // SAFETY: allocated with CoTaskMemAlloc by the endpoint.
    unsafe { CoTaskMemFree(Some(id.0 as *const _)) };
    text
}

fn capture(
    source: AudioSource,
    packets: &Sender<Packet>,
    ready: &SyncSender<Result<Info, String>>,
    stop: &AtomicBool,
) -> Result<(), String> {
    let opened = open(source);
    let (info, client, capture_client, render) = match opened {
        Ok(parts) => parts,
        Err(e) => {
            let _ = ready.send(Err(e));
            return Ok(());
        }
    };
    let (channels, format) = (info.channels, info.format);
    let _ = ready.send(Ok(info));

    if let Some((render_client, render_audio, size)) = &render {
        // Fill the whole buffer with silence, then start: the engine now has
        // a stream to mix on this endpoint for as long as we keep it fed.
        // SAFETY: the client is initialised; `size` is its buffer size.
        if unsafe { render_audio.GetBuffer(*size) }.is_ok() {
            // SAFETY: releases exactly the frames just obtained, as silence.
            let _ =
                unsafe { render_audio.ReleaseBuffer(*size, AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) };
        }
        // SAFETY: the client is initialised.
        unsafe { render_client.Start() }.map_err(|e| format!("keep-alive Start failed: {e}"))?;
    }
    // SAFETY: the client is initialised.
    unsafe { client.Start() }.map_err(|e| format!("IAudioClient::Start failed: {e}"))?;

    let result = pump(
        &capture_client,
        render.as_ref(),
        packets,
        stop,
        format,
        channels,
    );

    // SAFETY: both clients are live and started; stopping has no other
    // precondition.
    let _ = unsafe { client.Stop() };
    if let Some((render_client, _, _)) = &render {
        // SAFETY: as above.
        let _ = unsafe { render_client.Stop() };
    }
    result
}

type Opened = (
    Info,
    IAudioClient,
    IAudioCaptureClient,
    Option<(IAudioClient, IAudioRenderClient, u32)>,
);

fn open(source: AudioSource) -> Result<Opened, String> {
    // SAFETY: COM is initialised (MTA) on this thread.
    let enumerator: IMMDeviceEnumerator =
        unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
            .map_err(|e| format!("no device enumerator: {e}"))?;
    let (flow, name) = match source {
        AudioSource::Mic => (eCapture, "default microphone"),
        _ => (eRender, "default output, in loopback"),
    };
    // SAFETY: `enumerator` is live.
    let device = unsafe { enumerator.GetDefaultAudioEndpoint(flow, eConsole) }
        .map_err(|e| format!("no {name}: {e}"))?;
    // SAFETY: `device` is live; no activation parameters.
    let client: IAudioClient = unsafe { device.Activate(CLSCTX_ALL, None) }
        .map_err(|e| format!("could not activate the {name}: {e}"))?;
    // SAFETY: `client` is live; the returned pointer is freed below.
    let mix = unsafe { client.GetMixFormat() }.map_err(|e| format!("GetMixFormat failed: {e}"))?;
    let described = describe(mix);
    let loopback = source == AudioSource::System;
    // SAFETY: `mix` is a live format for the duration of the call.
    let init = unsafe {
        client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            if loopback {
                AUDCLNT_STREAMFLAGS_LOOPBACK
            } else {
                0
            },
            BUFFER_HNS,
            0,
            mix,
            None,
        )
    };

    let render = if loopback && init.is_ok() {
        // SAFETY: `device` is live.
        let render_client: IAudioClient = unsafe { device.Activate(CLSCTX_ALL, None) }
            .map_err(|e| format!("could not activate the keep-alive stream: {e}"))?;
        // SAFETY: `mix` is still live; it is freed after this block.
        unsafe { render_client.Initialize(AUDCLNT_SHAREMODE_SHARED, 0, BUFFER_HNS, 0, mix, None) }
            .map_err(|e| format!("keep-alive Initialize failed: {e}"))?;
        // SAFETY: initialised.
        let size = unsafe { render_client.GetBufferSize() }
            .map_err(|e| format!("GetBufferSize failed: {e}"))?;
        // SAFETY: initialised.
        let render_audio: IAudioRenderClient = unsafe { render_client.GetService() }
            .map_err(|e| format!("no IAudioRenderClient: {e}"))?;
        Some((render_client, render_audio, size))
    } else {
        None
    };

    // SAFETY: allocated by GetMixFormat with CoTaskMemAlloc, and no longer used.
    unsafe { CoTaskMemFree(Some(mix as *const _)) };
    let mix = described?;
    init.map_err(|e| format!("IAudioClient::Initialize ({name}) failed: {e}"))?;
    if !pcm::aac_rate_supported(mix.rate) {
        return Err(format!(
            "the {name} runs at {} Hz, and the AAC encoder takes only 44100 or 48000. Set the \
             device to 48000 Hz in Sound settings, or run with --audio none.",
            mix.rate
        ));
    }
    // SAFETY: initialised.
    let capture_client: IAudioCaptureClient =
        unsafe { client.GetService() }.map_err(|e| format!("no IAudioCaptureClient: {e}"))?;

    let info = Info {
        source: name,
        device_id: device_id(&device),
        rate: mix.rate,
        channels: mix.channels,
        format: mix.format,
        keep_alive: render.is_some(),
    };
    Ok((info, client, capture_client, render))
}

fn pump(
    capture_client: &IAudioCaptureClient,
    render: Option<&(IAudioClient, IAudioRenderClient, u32)>,
    packets: &Sender<Packet>,
    stop: &AtomicBool,
    format: SampleFormat,
    channels: u16,
) -> Result<(), String> {
    while !stop.load(Ordering::Acquire) {
        if let Some((render_client, render_audio, size)) = render {
            // SAFETY: the client is started.
            if let Ok(padding) = unsafe { render_client.GetCurrentPadding() } {
                let free = size.saturating_sub(padding);
                // SAFETY: `free` frames are available by the padding just read.
                if free > 0 && unsafe { render_audio.GetBuffer(free) }.is_ok() {
                    // SAFETY: releases exactly the frames obtained, as silence.
                    let _ = unsafe {
                        render_audio.ReleaseBuffer(free, AUDCLNT_BUFFERFLAGS_SILENT.0 as u32)
                    };
                }
            }
        }

        loop {
            // SAFETY: the client is started.
            let available = unsafe { capture_client.GetNextPacketSize() }
                .map_err(|e| format!("GetNextPacketSize failed: {e}"))?;
            if available == 0 {
                break;
            }
            let mut data: *mut u8 = std::ptr::null_mut();
            let mut frames = 0u32;
            let mut flags = 0u32;
            let mut qpc = 0u64;
            // SAFETY: every out-parameter is live; released below.
            unsafe {
                capture_client.GetBuffer(&mut data, &mut frames, &mut flags, None, Some(&mut qpc))
            }
            .map_err(|e| format!("GetBuffer failed: {e}"))?;
            let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0;
            let pcm = if silent || data.is_null() {
                vec![0i16; frames as usize * 2]
            } else {
                let bytes = frames as usize * usize::from(channels) * format.bytes();
                // SAFETY: the engine owns `bytes` bytes at `data` until
                // ReleaseBuffer, in the mix format Initialize accepted.
                let raw = unsafe { std::slice::from_raw_parts(data, bytes) };
                pcm::to_stereo_i16(raw, format, channels, frames)
            };
            // SAFETY: releases exactly the packet GetBuffer handed out.
            unsafe { capture_client.ReleaseBuffer(frames) }
                .map_err(|e| format!("ReleaseBuffer failed: {e}"))?;
            let packet = Packet {
                // WASAPI's QPC position is already in 100 ns units.
                qpc_hns: qpc as i64,
                frames,
                discontinuity: flags & AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY.0 as u32 != 0,
                pcm,
            };
            if packets.send(packet).is_err() {
                return Ok(());
            }
        }
        std::thread::sleep(POLL);
    }
    Ok(())
}
