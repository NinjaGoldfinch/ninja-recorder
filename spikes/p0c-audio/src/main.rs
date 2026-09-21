//! **P0c stage 1 (WS1.3, #7): can process loopback isolate game audio?**
//!
//! **Not a licence trade, though #7 and #67 were both written as though it
//! were.** The premise was that failing here means keeping libobs to keep
//! per-application audio. It does not: the fork captures per-app audio with
//! `wasapi_process_output_capture`, which is OBS's process-loopback source,
//! which is the same `ActivateAudioInterfaceAsync` +
//! `AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK` this file calls. There is no
//! fallback inside it either. So if this fails, it fails for both backends and
//! keeping libobs saves nothing. #67 has the evidence.
//!
//! What that leaves is a question about Windows rather than about our code:
//! does process loopback work against a Vanguard-protected process? This spike
//! answers it in isolation, away from a capture pipeline that could be blamed
//! for the result.
//!
//! The spike answers exactly that and nothing else: point it at a process,
//! record for a while, write a WAV. Play the WAV. If the game is in it and
//! Discord is not, stage 1 passes.
//!
//! ## Running it
//!
//! ```text
//! cargo run --release -- --pid 1234 --seconds 60 --out game.wav
//! cargo run --release -- --name "League of Legends.exe" --seconds 60
//! cargo run --release -- --list
//! ```
//!
//! The exit criterion (#7) is "game audio present, Discord absent, root PID
//! documented", so run it with a game **and** Discord both making noise. A WAV
//! with both in it is a failure with a clear meaning, not an inconclusive run.
//!
//! ## What it is deliberately not
//!
//! Not a recorder. There is no encoder, no container, no device switching and
//! no error recovery: it writes 32-bit float PCM into a RIFF file and stops.
//! Everything it leaves out is something `recorder/own/` will have to do, and
//! doing any of it here would make a failure ambiguous between "loopback does
//! not isolate" and "the spike is wrong", which is the one distinction the
//! whole exercise needs to keep clean.

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!(
        "p0c-audio captures WASAPI process loopback, which exists only on Windows.\n\
         It is checked on other platforms (cargo check --target x86_64-pc-windows-msvc)\n\
         and run on the box."
    );
    std::process::exit(2);
}

#[cfg(target_os = "windows")]
fn main() -> std::process::ExitCode {
    match windows_impl::run() {
        Ok(report) => {
            println!("{report}");
            std::process::ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("p0c-audio: {err}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(target_os = "windows")]
mod windows_impl {
    use std::fmt::Write as _;
    use std::fs::File;
    use std::io::{BufWriter, Seek, SeekFrom, Write};
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    use windows::core::{implement, Interface, Ref, Result as WinResult, PCWSTR};
    use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
    use windows::Win32::Media::Audio::{
        eRender, IAudioCaptureClient, IAudioClient, IActivateAudioInterfaceAsyncOperation,
        IActivateAudioInterfaceCompletionHandler, IActivateAudioInterfaceCompletionHandler_Impl,
        IMMDeviceEnumerator, MMDeviceEnumerator, AUDCLNT_SHAREMODE_SHARED,
        AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_LOOPBACK,
        AUDIOCLIENT_ACTIVATION_PARAMS, AUDIOCLIENT_ACTIVATION_PARAMS_0,
        AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK, AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS,
        AUDCLNT_BUFFERFLAGS_SILENT, DEVICE_STATE_ACTIVE,
        PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE, VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
        WAVEFORMATEX,
    };
    use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
    use windows::Win32::System::Com::{
        CoInitializeEx, CoUninitialize, BLOB, COINIT_MULTITHREADED,
    };
    use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
    use windows::Win32::System::Variant::VT_BLOB;

    /// `WAVE_FORMAT_IEEE_FLOAT`, spelled out rather than imported: windows-rs
    /// puts it behind `Win32_Media_KernelStreaming`, and pulling in a whole
    /// feature for one documented constant buys nothing.
    const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;

    /// The format we ask for.
    ///
    /// **Process loopback gives no `GetMixFormat`**, which is the first
    /// surprise in this API: with a real endpoint you ask the device what it
    /// is doing and match it, and here there is no device to ask. The format
    /// below is what we assert, and the engine converts into it. 48 kHz
    /// stereo float is what the mix graph is natively doing on every machine
    /// this will ever run on, so it is the cheapest thing to ask for.
    const SAMPLE_RATE: u32 = 48_000;
    const CHANNELS: u16 = 2;
    const BITS: u16 = 32;

    /// How long `WaitForSingleObject` waits on the capture event before
    /// deciding the engine has stopped feeding us. Generous: a stall is a
    /// finding, and reporting one wrongly would be worse than waiting.
    const EVENT_TIMEOUT_MS: u32 = 2_000;

    pub struct Args {
        pub target: Target,
        pub seconds: u64,
        pub out: PathBuf,
    }

    pub enum Target {
        Pid(u32),
        Name(String),
        List,
    }

    pub fn run() -> Result<String, String> {
        let args = parse_args()?;

        // MTA for the duration. `recorder/devices.rs` owns a thread for the
        // same reason: an STA thread that tried this would get
        // RPC_E_CHANGED_MODE. This binary has no UI, so its main thread is
        // free to be MTA.
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
            .ok()
            .map_err(|e| format!("CoInitializeEx failed: {e}"))?;
        let result = run_initialized(&args);
        unsafe { CoUninitialize() };
        result
    }

    fn run_initialized(args: &Args) -> Result<String, String> {
        match &args.target {
            Target::List => list_render_endpoints(),
            Target::Pid(pid) => capture(*pid, args),
            Target::Name(name) => {
                Err(format!(
                    "--name is not resolved by this spike: pass --pid instead.\n\
                     `Get-Process -Name '{}' | Select-Object Id, ProcessName` gives it, and the\n\
                     PID is part of what #7 asks to be documented anyway.",
                    name.trim_end_matches(".exe")
                ))
            }
        }
    }

    /// Not part of the exit criterion; it is here because the first question
    /// on a machine where nothing is captured is always "is anything playing
    /// at all", and answering it without a second tool saves a round trip.
    fn list_render_endpoints() -> Result<String, String> {
        unsafe {
            let enumerator: IMMDeviceEnumerator =
                windows::Win32::System::Com::CoCreateInstance(
                    &MMDeviceEnumerator,
                    None,
                    windows::Win32::System::Com::CLSCTX_ALL,
                )
                .map_err(|e| format!("could not create the device enumerator: {e}"))?;
            let collection = enumerator
                .EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)
                .map_err(|e| format!("could not enumerate render endpoints: {e}"))?;
            let count = collection
                .GetCount()
                .map_err(|e| format!("could not count render endpoints: {e}"))?;

            let mut out = String::new();
            let _ = writeln!(out, "{count} active render endpoint(s).");
            let _ = writeln!(
                out,
                "Process loopback does not target one of these: it targets a PID, and the\n\
                 endpoint it mixes through is chosen for us. This is only here to confirm\n\
                 the machine has working audio before reading anything into an empty WAV."
            );
            Ok(out)
        }
    }

    /// The completion handler `ActivateAudioInterfaceAsync` calls.
    ///
    /// The activation is asynchronous and there is no synchronous form, so the
    /// call below hands over this object and then blocks on an event it
    /// signals. The handler itself does nothing but signal: the result is read
    /// from the operation afterwards, on the calling thread, where the error
    /// can be reported in the one place that knows what was being attempted.
    #[implement(IActivateAudioInterfaceCompletionHandler)]
    struct ActivationHandler(HANDLE);

    impl IActivateAudioInterfaceCompletionHandler_Impl for ActivationHandler_Impl {
        fn ActivateCompleted(
            &self,
            _operation: Ref<IActivateAudioInterfaceAsyncOperation>,
        ) -> WinResult<()> {
            unsafe { windows::Win32::System::Threading::SetEvent(self.0) }
        }
    }

    /// An owned Win32 event handle, closed on drop.
    ///
    /// Two of these are live at once and one of them is handed to a COM object
    /// that outlives the stack frame that made it, which is exactly the shape
    /// that leaks a handle per run when the closing is written by hand at each
    /// early return.
    struct Event(HANDLE);

    impl Event {
        fn new() -> Result<Self, String> {
            let handle = unsafe { CreateEventW(None, false, false, PCWSTR::null()) }
                .map_err(|e| format!("CreateEventW failed: {e}"))?;
            Ok(Self(handle))
        }
    }

    impl Drop for Event {
        fn drop(&mut self) {
            if !self.0.is_invalid() {
                let _ = unsafe { CloseHandle(self.0) };
            }
        }
    }

    fn capture(pid: u32, args: &Args) -> Result<String, String> {
        let activated = Event::new()?;
        let client = unsafe { activate_for_pid(pid, activated.0) }?;

        let format = float_format();
        // Event-driven: the engine signals when a buffer is ready rather than
        // us polling a period. A poll would work for a spike, but the real
        // backend will be event-driven and a difference here would make the
        // spike's timing say nothing about the thing it is standing in for.
        //
        // `AUDCLNT_STREAMFLAGS_LOOPBACK` is required even though the
        // activation already said "process loopback": the flag is what makes
        // the client a capture client on a render stream.
        unsafe {
            client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                // Buffer duration and periodicity must both be 0 for a process
                // loopback client. Anything else fails with E_INVALIDARG, and
                // the error does not say which argument.
                0,
                0,
                &format,
                None,
            )
        }
        .map_err(|e| {
            format!(
                "IAudioClient::Initialize failed: {e}\n\
                 E_INVALIDARG here usually means the buffer duration or periodicity was not 0,\n\
                 or the format asked for is not {SAMPLE_RATE} Hz / {CHANNELS} ch / {BITS}-bit float."
            )
        })?;

        let ready = Event::new()?;
        unsafe { client.SetEventHandle(ready.0) }
            .map_err(|e| format!("SetEventHandle failed: {e}"))?;

        let capture_client: IAudioCaptureClient = unsafe { client.GetService() }
            .map_err(|e| format!("could not get IAudioCaptureClient: {e}"))?;

        let mut wav = WavWriter::create(&args.out, SAMPLE_RATE, CHANNELS, BITS)?;

        unsafe { client.Start() }.map_err(|e| format!("IAudioClient::Start failed: {e}"))?;
        let outcome = pump(&capture_client, &ready, &mut wav, args.seconds);
        let _ = unsafe { client.Stop() };

        let stats = outcome?;
        let bytes = wav.finish()?;

        let mut report = String::new();
        let _ = writeln!(report, "Captured {pid} for {}s.", args.seconds);
        let _ = writeln!(report, "  file          {}", args.out.display());
        let _ = writeln!(report, "  format        {SAMPLE_RATE} Hz, {CHANNELS} ch, {BITS}-bit float");
        let _ = writeln!(report, "  frames        {}", stats.frames);
        let _ = writeln!(report, "  bytes on disk {bytes}");
        let _ = writeln!(report, "  silent packets {} of {}", stats.silent, stats.packets);
        let _ = writeln!(report, "  peak amplitude {:.6}", stats.peak);
        let _ = writeln!(report);
        let _ = writeln!(report, "{}", verdict(&stats));
        Ok(report)
    }

    /// What the numbers mean, said here rather than left to whoever reads them.
    ///
    /// A run that captures nothing and a run that captures silence are
    /// different findings with the same file size, and telling them apart
    /// later from a WAV is harder than saying so now.
    fn verdict(stats: &Stats) -> String {
        if stats.frames == 0 {
            return "NOTHING CAPTURED. The activation succeeded and the engine produced no \
                    frames at all.\nThat is either a process making no sound, or a process \
                    whose audio does not route through\nthe loopback we attached to. Check the \
                    target was audible before reading this as a failure."
                .to_string();
        }
        if stats.peak == 0.0 {
            return "SILENCE CAPTURED. Frames arrived and every sample was zero.\nThis is the \
                    interesting failure: the stream exists, so the activation and the format \
                    are right,\nand the target's audio is not reaching it."
                .to_string();
        }
        "AUDIO CAPTURED. Now play the file: stage 1 passes only if the target is in it \
         and everything\nelse on the machine is not. The numbers above cannot tell you that; \
         only listening can."
            .to_string()
    }

    struct Stats {
        frames: u64,
        packets: u64,
        silent: u64,
        peak: f32,
    }

    fn pump(
        capture_client: &IAudioCaptureClient,
        ready: &Event,
        wav: &mut WavWriter,
        seconds: u64,
    ) -> Result<Stats, String> {
        let deadline = Instant::now() + Duration::from_secs(seconds);
        let mut stats = Stats {
            frames: 0,
            packets: 0,
            silent: 0,
            peak: 0.0,
        };

        while Instant::now() < deadline {
            let wait = unsafe { WaitForSingleObject(ready.0, EVENT_TIMEOUT_MS) };
            if wait != WAIT_OBJECT_0 {
                return Err(format!(
                    "the capture event stopped being signalled after {} frame(s).\n\
                     The engine was feeding us and then stopped, which is worth reporting as \
                     itself rather than\nas a short file.",
                    stats.frames
                ));
            }

            // One event can cover several packets. Draining is not an
            // optimisation: leaving a packet behind makes the next
            // `GetBuffer` return it late and the capture drift.
            loop {
                let available = unsafe { capture_client.GetNextPacketSize() }
                    .map_err(|e| format!("GetNextPacketSize failed: {e}"))?;
                if available == 0 {
                    break;
                }

                let mut data: *mut u8 = std::ptr::null_mut();
                let mut frames: u32 = 0;
                let mut flags: u32 = 0;
                unsafe {
                    capture_client.GetBuffer(&mut data, &mut frames, &mut flags, None, None)
                }
                .map_err(|e| format!("GetBuffer failed: {e}"))?;

                stats.packets += 1;
                let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0;
                if silent {
                    stats.silent += 1;
                }

                if frames > 0 {
                    let samples = frames as usize * CHANNELS as usize;
                    // SAFETY: the engine owns this buffer until ReleaseBuffer
                    // and promises `frames * channels` samples of the format
                    // Initialize accepted, which is 32-bit float above.
                    let pcm = unsafe { std::slice::from_raw_parts(data as *const f32, samples) };
                    if silent {
                        // A silent packet's buffer contents are undefined
                        // rather than zero. Writing it through would put noise
                        // in the file and the peak would stop meaning anything.
                        wav.write_silence(samples)?;
                    } else {
                        for &sample in pcm {
                            let magnitude = sample.abs();
                            if magnitude > stats.peak {
                                stats.peak = magnitude;
                            }
                        }
                        wav.write_samples(pcm)?;
                    }
                    stats.frames += u64::from(frames);
                }

                unsafe { capture_client.ReleaseBuffer(frames) }
                    .map_err(|e| format!("ReleaseBuffer failed: {e}"))?;
            }
        }

        Ok(stats)
    }

    /// Activates an `IAudioClient` bound to one process tree.
    ///
    /// # Safety
    ///
    /// `activated` must be a valid manual-or-auto reset event that nothing
    /// else is waiting on; the handler signals it exactly once.
    unsafe fn activate_for_pid(pid: u32, activated: HANDLE) -> Result<IAudioClient, String> {
        let mut params = AUDIOCLIENT_ACTIVATION_PARAMS {
            ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
            Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
                ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                    TargetProcessId: pid,
                    // **The tree, not the process.** A modern game is several
                    // processes and the one that plays audio is often not the
                    // one that owns the window; League in particular runs the
                    // client and the game as separate executables. Targeting a
                    // single PID is how this spike would produce a false
                    // negative.
                    ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
                },
            },
        };

        // The activation parameters travel inside a PROPVARIANT as a raw blob.
        // Built by hand because there is no constructor for VT_BLOB: the
        // union's layout is what the API reads, and `..Default::default()`
        // zeroes the rest.
        let mut variant = PROPVARIANT::default();
        // SAFETY: writing the active arm of the union, which is what the
        // callee reads given `vt = VT_BLOB`.
        unsafe {
            let inner = &mut variant.Anonymous.Anonymous;
            inner.vt = VT_BLOB;
            inner.Anonymous.blob = BLOB {
                cbSize: size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32,
                pBlobData: (&raw mut params).cast::<u8>(),
            };
        }

        let handler: IActivateAudioInterfaceCompletionHandler =
            ActivationHandler(activated).into();

        let operation = unsafe {
            windows::Win32::Media::Audio::ActivateAudioInterfaceAsync(
                VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
                &IAudioClient::IID,
                Some(&raw const variant),
                &handler,
            )
        }
        .map_err(|e| format!("ActivateAudioInterfaceAsync failed: {e}"))?;

        // Blocking on the handler's event. There is no synchronous activation.
        let wait = unsafe { WaitForSingleObject(activated, EVENT_TIMEOUT_MS) };
        if wait != WAIT_OBJECT_0 {
            return Err(
                "the activation never completed. Nothing in this API reports why, so the \
                 next thing to check\nis whether the PID still exists."
                    .to_string(),
            );
        }

        let mut activate_result = windows::core::HRESULT(0);
        let mut interface: Option<windows::core::IUnknown> = None;
        unsafe { operation.GetActivateResult(&mut activate_result, &mut interface) }
            .map_err(|e| format!("GetActivateResult failed: {e}"))?;
        activate_result.ok().map_err(|e| {
            format!(
                "activation was refused: {e}\n\
                 E_ACCESSDENIED here is the finding, not a bug: it means this build of Windows \
                 will not\nlet an unelevated process capture that target."
            )
        })?;

        interface
            .ok_or_else(|| "activation succeeded and returned no interface".to_string())?
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

    // --- WAV -------------------------------------------------------------

    /// A RIFF writer, and nothing more.
    ///
    /// Float WAV rather than 16-bit PCM so that nothing in the spike can be
    /// blamed for what the file sounds like: there is no conversion between
    /// what the engine produced and what lands on disk.
    struct WavWriter {
        file: BufWriter<File>,
        data_bytes: u32,
    }

    impl WavWriter {
        fn create(path: &PathBuf, rate: u32, channels: u16, bits: u16) -> Result<Self, String> {
            let file = File::create(path)
                .map_err(|e| format!("could not create {}: {e}", path.display()))?;
            let mut writer = Self {
                file: BufWriter::new(file),
                data_bytes: 0,
            };
            writer.write_header(rate, channels, bits)?;
            Ok(writer)
        }

        fn write_header(&mut self, rate: u32, channels: u16, bits: u16) -> Result<(), String> {
            let block_align = channels * bits / 8;
            let mut header = Vec::with_capacity(44);
            header.extend_from_slice(b"RIFF");
            header.extend_from_slice(&0u32.to_le_bytes()); // patched in finish()
            header.extend_from_slice(b"WAVEfmt ");
            header.extend_from_slice(&16u32.to_le_bytes());
            header.extend_from_slice(&WAVE_FORMAT_IEEE_FLOAT.to_le_bytes());
            header.extend_from_slice(&channels.to_le_bytes());
            header.extend_from_slice(&rate.to_le_bytes());
            header.extend_from_slice(&(rate * u32::from(block_align)).to_le_bytes());
            header.extend_from_slice(&block_align.to_le_bytes());
            header.extend_from_slice(&bits.to_le_bytes());
            header.extend_from_slice(b"data");
            header.extend_from_slice(&0u32.to_le_bytes()); // patched in finish()
            self.file
                .write_all(&header)
                .map_err(|e| format!("could not write the WAV header: {e}"))
        }

        fn write_samples(&mut self, samples: &[f32]) -> Result<(), String> {
            for &sample in samples {
                self.file
                    .write_all(&sample.to_le_bytes())
                    .map_err(|e| format!("could not write samples: {e}"))?;
            }
            self.data_bytes += (samples.len() * 4) as u32;
            Ok(())
        }

        fn write_silence(&mut self, samples: usize) -> Result<(), String> {
            let zeros = vec![0u8; samples * 4];
            self.file
                .write_all(&zeros)
                .map_err(|e| format!("could not write silence: {e}"))?;
            self.data_bytes += zeros.len() as u32;
            Ok(())
        }

        /// Patches the two sizes RIFF puts before the data it describes.
        fn finish(mut self) -> Result<u32, String> {
            self.file
                .flush()
                .map_err(|e| format!("could not flush the WAV: {e}"))?;
            let mut file = self
                .file
                .into_inner()
                .map_err(|e| format!("could not finish the WAV: {e}"))?;

            file.seek(SeekFrom::Start(4))
                .and_then(|_| file.write_all(&(36 + self.data_bytes).to_le_bytes()))
                .and_then(|()| file.seek(SeekFrom::Start(40)).map(|_| ()))
                .and_then(|()| file.write_all(&self.data_bytes.to_le_bytes()))
                .map_err(|e| format!("could not patch the WAV sizes: {e}"))?;
            Ok(self.data_bytes + 44)
        }
    }

    // --- Arguments -------------------------------------------------------

    fn parse_args() -> Result<Args, String> {
        let mut target = None;
        let mut seconds = 60u64;
        let mut out = PathBuf::from("p0c-audio.wav");

        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--list" => target = Some(Target::List),
                "--pid" => {
                    let value = args.next().ok_or("--pid needs a process id")?;
                    let pid = value
                        .parse::<u32>()
                        .map_err(|_| format!("--pid wants a number, got {value:?}"))?;
                    target = Some(Target::Pid(pid));
                }
                "--name" => {
                    let value = args.next().ok_or("--name needs a process name")?;
                    target = Some(Target::Name(value));
                }
                "--seconds" => {
                    let value = args.next().ok_or("--seconds needs a number")?;
                    seconds = value
                        .parse::<u64>()
                        .map_err(|_| format!("--seconds wants a number, got {value:?}"))?;
                }
                "--out" => {
                    out = PathBuf::from(args.next().ok_or("--out needs a path")?);
                }
                "--help" | "-h" => return Err(usage()),
                other => return Err(format!("unknown argument {other:?}\n\n{}", usage())),
            }
        }

        Ok(Args {
            target: target.ok_or_else(|| format!("nothing to capture.\n\n{}", usage()))?,
            seconds,
            out,
        })
    }

    fn usage() -> String {
        "p0c-audio — WS1.3 (#7), the process-loopback spike\n\
         \n\
           --pid <n>         the process to capture, with its children\n\
           --seconds <n>     how long for (default 60)\n\
           --out <path>      where the WAV goes (default p0c-audio.wav)\n\
           --list            confirm the machine has working audio output\n\
         \n\
         Run it with the game and Discord both making noise: the exit criterion is\n\
         that one of them is in the file and the other is not."
            .to_string()
    }
}
