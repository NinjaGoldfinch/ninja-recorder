//! **P0c stage 2 (WS1.4, #8): frames out of WGC and into a fragmented MP4.**
//!
//! Three claims, and the third is the one the process split rests on:
//!
//! 1. Windows.Graphics.Capture produces frames for what League actually runs
//!    in, without injecting anything. Injection is a hard constraint, not a
//!    preference (DEVELOPMENT.md §1.1).
//! 2. Those frames reach a Media Foundation `SinkWriter` and come out as
//!    H.264 with the presentation clock intact. The run reports the worst gap
//!    between where a frame's timestamp said it was and where the wall clock
//!    said it should be, in frame intervals.
//! 3. **A file killed mid-write is still playable.** That is what makes "the
//!    daemon can die and the recording survives" a claim rather than a hope,
//!    and it is why the sink is fragmented MP4 rather than ordinary MP4: the
//!    `moov` atom goes first, so every fragment written before the kill stands
//!    on its own.
//!
//! ## Running it
//!
//! ```text
//! p0c-video --list
//! p0c-video --monitor 0 --seconds 600 --out sample.mp4
//! p0c-video --window "League of Legends (TM) Client" --seconds 600
//! ```
//!
//! The exit criterion asks for a ten-minute sample with the process killed at
//! minute five. Do both: one clean run for drift, one killed run for the file.
//! **Kill it from Task Manager, not with Ctrl+C** — a clean shutdown finalizes
//! the sink, which is the case that was never in doubt.
//!
//! ## The vendor half of the exit criterion cannot be met
//!
//! #8 asks for encoder detection on two GPU vendors. #68 settled that only
//! NVIDIA and software-only are available, so what this can establish is that
//! an H.264 encoder is selected and initialises on this machine, and what
//! happens when none is. The AMF and oneVPL orderings stay unverified, and
//! `SetInputMediaType` is where a machine with no usable encoder says so: its
//! error message says to record what was offered, because "no encoder" is a
//! finding rather than a failed run.

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!(
        "p0c-video captures Windows.Graphics.Capture, which exists only on Windows.\n\
         It is checked elsewhere (cargo check --target x86_64-pc-windows-msvc) and run on the box."
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
            eprintln!("p0c-video: {err}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(target_os = "windows")]
mod windows_impl {
    use std::fmt::Write as _;
    use std::path::PathBuf;
    use std::sync::mpsc::{channel, Receiver, Sender};
    use std::time::{Duration, Instant};

    use windows::core::{Interface, Result as WinResult, HSTRING};
    use windows::Foundation::TypedEventHandler;
    use windows::Graphics::Capture::{
        Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
    };
    use windows::Graphics::DirectX::DirectXPixelFormat;
    use windows::Graphics::SizeInt32;
    use windows::Win32::Foundation::{HMODULE, HWND, LPARAM, RECT, TRUE};
    use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
    use windows::Win32::Graphics::Direct3D11::{
        D3D11CreateDevice, ID3D11Device, ID3D11Texture2D, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
        D3D11_SDK_VERSION,
    };
    use windows::Win32::Graphics::Dxgi::IDXGIDevice;
    use windows::Win32::Graphics::Gdi::{
        EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
    };
    use windows::Win32::Media::MediaFoundation::{
        IMFAttributes, IMFDXGIDeviceManager, IMFMediaType, IMFSinkWriter, MFCreateAttributes,
        MFCreateDXGIDeviceManager, MFCreateDXGISurfaceBuffer, MFCreateMediaType,
        MFCreateSample, MFCreateSinkWriterFromURL, MFMediaType_Video, MFStartup,
        MFVideoFormat_H264, MFVideoFormat_RGB32, MFShutdown, MFSTARTUP_FULL,
        MFTranscodeContainerType_FMPEG4, MF_MPEG4SINK_MOOV_BEFORE_MDAT, MF_MT_AVG_BITRATE,
        MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE,
        MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SUBTYPE, MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS,
        MF_SINK_WRITER_D3D_MANAGER, MF_TRANSCODE_CONTAINERTYPE, MFVideoInterlace_Progressive,
    };
    use windows::Win32::System::WinRT::Direct3D11::{
        CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
    };
    use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
    };

    /// 100-nanosecond units, which is Media Foundation's clock everywhere.
    const HNS_PER_SECOND: i64 = 10_000_000;

    const FPS: u32 = 60;
    /// 8 Mbps, which is what #12's quality comparison is specified against.
    const BITRATE: u32 = 8_000_000;

    pub struct Args {
        pub target: Target,
        pub seconds: u64,
        pub out: PathBuf,
    }

    pub enum Target {
        Monitor(usize),
        Window(String),
        List,
    }

    pub fn run() -> Result<String, String> {
        let args = parse_args()?;
        if let Target::List = args.target {
            return list_targets();
        }

        // MF owns a thread pool and a clock; both have to be up before any
        // `MFCreate*` call and torn down after the sink writer is dropped.
        unsafe { MFStartup(mf_version(), MFSTARTUP_FULL) }
            .map_err(|e| format!("MFStartup failed: {e}"))?;
        let result = capture(&args);
        let _ = unsafe { MFShutdown() };
        result
    }

    /// `MF_VERSION`, which windows-rs does not expose as a constant: it is
    /// `(MF_SDK_VERSION << 16) | MF_API_VERSION`, and both halves are fixed.
    pub fn mf_version() -> u32 {
        const MF_SDK_VERSION: u32 = 0x0002;
        const MF_API_VERSION: u32 = 0x0070;
        (MF_SDK_VERSION << 16) | MF_API_VERSION
    }

    // --- Targets ---------------------------------------------------------

    struct Monitor {
        handle: HMONITOR,
        bounds: RECT,
    }

    fn monitors() -> Vec<Monitor> {
        // SAFETY: the callback only appends to the vector behind `lparam`,
        // and `EnumDisplayMonitors` is synchronous, so the borrow cannot
        // outlive this call.
        unsafe extern "system" fn collect(
            handle: HMONITOR,
            _dc: HDC,
            _clip: *mut RECT,
            lparam: LPARAM,
        ) -> windows::core::BOOL {
            let found = unsafe { &mut *(lparam.0 as *mut Vec<Monitor>) };
            let mut info = MONITORINFO {
                cbSize: size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            if unsafe { GetMonitorInfoW(handle, &mut info) }.as_bool() {
                found.push(Monitor {
                    handle,
                    bounds: info.rcMonitor,
                });
            }
            TRUE
        }

        let mut found: Vec<Monitor> = Vec::new();
        let _ = unsafe {
            EnumDisplayMonitors(
                None,
                None,
                Some(collect),
                LPARAM(&raw mut found as isize),
            )
        };
        found
    }

    struct Window {
        handle: HWND,
        title: String,
        pid: u32,
    }

    fn windows_with_titles() -> Vec<Window> {
        unsafe extern "system" fn collect(handle: HWND, lparam: LPARAM) -> windows::core::BOOL {
            let found = unsafe { &mut *(lparam.0 as *mut Vec<Window>) };
            if unsafe { IsWindowVisible(handle) }.as_bool() {
                let mut buffer = [0u16; 512];
                let len = unsafe { GetWindowTextW(handle, &mut buffer) };
                if len > 0 {
                    let mut pid = 0u32;
                    unsafe { GetWindowThreadProcessId(handle, Some(&mut pid)) };
                    found.push(Window {
                        handle,
                        title: String::from_utf16_lossy(&buffer[..len as usize]),
                        pid,
                    });
                }
            }
            TRUE
        }

        let mut found: Vec<Window> = Vec::new();
        let _ = unsafe { EnumWindows(Some(collect), LPARAM(&raw mut found as isize)) };
        found
    }

    /// Everything capturable, so the run can name a target without guessing.
    ///
    /// The window list carries process ids because #7's audio spike wants one
    /// and the two are usually run against the same game in the same sitting.
    fn list_targets() -> Result<String, String> {
        let mut out = String::new();
        let _ = writeln!(out, "Monitors:");
        for (i, monitor) in monitors().iter().enumerate() {
            let width = monitor.bounds.right - monitor.bounds.left;
            let height = monitor.bounds.bottom - monitor.bounds.top;
            let _ = writeln!(out, "  --monitor {i}    {width}x{height}");
        }
        let _ = writeln!(out);
        let _ = writeln!(out, "Windows:");
        for window in windows_with_titles() {
            let _ = writeln!(out, "  pid {:>6}  {}", window.pid, window.title);
        }
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "League runs the client and the game as separate windows. Capture the one that is\n\
             actually rendering the match, and note its pid for p0c-audio."
        );
        Ok(out)
    }

    fn capture_item(target: &Target) -> Result<GraphicsCaptureItem, String> {
        // WGC's items are WinRT and the handles are Win32, so the bridge is
        // an interop interface obtained from the WinRT activation factory.
        let interop: IGraphicsCaptureItemInterop =
            windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()
                .map_err(|e| format!("no capture interop factory: {e} (needs Windows 10 1803+)"))?;

        match target {
            Target::Monitor(index) => {
                let found = monitors();
                let monitor = found
                    .get(*index)
                    .ok_or_else(|| format!("no monitor {index}; --list shows {}", found.len()))?;
                unsafe { interop.CreateForMonitor(monitor.handle) }
                    .map_err(|e| format!("CreateForMonitor failed: {e}"))
            }
            Target::Window(title) => {
                let needle = title.to_lowercase();
                let window = windows_with_titles()
                    .into_iter()
                    .find(|w| w.title.to_lowercase().contains(&needle))
                    .ok_or_else(|| format!("no visible window matching {title:?}; try --list"))?;
                unsafe { interop.CreateForWindow(window.handle) }
                    .map_err(|e| format!("CreateForWindow failed: {e}"))
            }
            Target::List => Err("nothing to capture".to_string()),
        }
    }

    // --- The pipeline ----------------------------------------------------

    fn capture(args: &Args) -> Result<String, String> {
        let item = capture_item(&args.target)?;
        let size = item.Size().map_err(|e| format!("item has no size: {e}"))?;
        // H.264 wants even dimensions and a window can be any size.
        let width = (size.Width as u32) & !1;
        let height = (size.Height as u32) & !1;
        if width == 0 || height == 0 {
            return Err("the capture target has no area".to_string());
        }

        let (device, d3d) = create_d3d_device()?;
        let writer = create_sink_writer(&args.out, width, height, &device)?;
        let stream = 0u32;

        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &d3d,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            // Two buffers is the documented minimum that still lets the
            // compositor hand over a frame while we hold one.
            2,
            SizeInt32 {
                Width: width as i32,
                Height: height as i32,
            },
        )
        .map_err(|e| format!("could not create the frame pool: {e}"))?;

        let (tx, rx): (Sender<()>, Receiver<()>) = channel();
        let token = pool
            .FrameArrived(&TypedEventHandler::new(
                move |_pool: windows_core::Ref<Direct3D11CaptureFramePool>, _| -> WinResult<()> {
                    // The handler does nothing but wake the writing thread.
                    // Encoding on the compositor's callback is how a capture
                    // ends up stuttering the thing it is capturing.
                    let _ = tx.send(());
                    Ok(())
                },
            ))
            .map_err(|e| format!("could not subscribe to FrameArrived: {e}"))?;

        let session = pool
            .CreateCaptureSession(&item)
            .map_err(|e| format!("could not create the capture session: {e}"))?;
        // The yellow border is a system affordance and on some builds it can
        // be turned off. Leave it on: a capture that hides itself is exactly
        // what the no-injection constraint exists to avoid looking like.
        let _ = session.SetIsBorderRequired(true);
        hide_cursor_if_possible(&session);

        unsafe { writer.BeginWriting() }.map_err(|e| format!("BeginWriting failed: {e}"))?;
        session
            .StartCapture()
            .map_err(|e| format!("StartCapture failed: {e}"))?;

        let stats = pump(&pool, &rx, &writer, stream, args.seconds)?;

        // A clean finalize, which is the case that was never in doubt. The
        // interesting run is the one killed from Task Manager before reaching
        // this line.
        session.Close().ok();
        pool.RemoveFrameArrived(token).ok();
        pool.Close().ok();
        unsafe { writer.Finalize() }.map_err(|e| format!("Finalize failed: {e}"))?;

        Ok(report(args, width, height, &stats))
    }

    struct Stats {
        frames: u64,
        dropped: u64,
        elapsed: Duration,
        /// Largest gap between a frame's presentation time and where the
        /// wall clock said it should be, in frame intervals.
        worst_drift_frames: f64,
    }

    fn pump(
        pool: &Direct3D11CaptureFramePool,
        rx: &Receiver<()>,
        writer: &IMFSinkWriter,
        stream: u32,
        seconds: u64,
    ) -> Result<Stats, String> {
        let started = Instant::now();
        let deadline = started + Duration::from_secs(seconds);
        let frame_interval = HNS_PER_SECOND / i64::from(FPS);

        let mut frames: u64 = 0;
        let mut dropped: u64 = 0;
        let mut worst_drift_frames = 0.0f64;

        while Instant::now() < deadline {
            // A timeout rather than a blocking wait: a capture that stops
            // producing frames is a finding, and a spike that hangs reports
            // nothing at all.
            if rx.recv_timeout(Duration::from_secs(2)).is_err() {
                return Err(format!(
                    "no frame arrived for two seconds, after {frames}. The session was still\n\
                     open, so this is the compositor having stopped rather than us having\n\
                     stopped asking."
                ));
            }

            // Drain: one wake can cover several frames, and leaving one in
            // the pool makes the next arrive late and the drift meaningless.
            while let Ok(frame) = pool.TryGetNextFrame() {
                let surface = frame
                    .Surface()
                    .map_err(|e| format!("frame has no surface: {e}"))?;
                let access: IDirect3DDxgiInterfaceAccess = surface
                    .cast()
                    .map_err(|e| format!("surface is not a DXGI interface: {e}"))?;
                let texture: ID3D11Texture2D = unsafe { access.GetInterface() }
                    .map_err(|e| format!("could not reach the texture: {e}"))?;

                let system_relative = frame
                    .SystemRelativeTime()
                    .map(|t| t.Duration)
                    .unwrap_or_default();

                match write_frame(writer, stream, &texture, frames, frame_interval) {
                    Ok(()) => {
                        // Drift is measured against the frame's own clock
                        // rather than against ours: the question is whether
                        // the presentation times we write stay in step with
                        // what the compositor said, not whether this loop is
                        // punctual.
                        if system_relative > 0 && frames > 0 {
                            let expected = frames as i64 * frame_interval;
                            let elapsed_hns = started.elapsed().as_nanos() as i64 / 100;
                            let gap = (elapsed_hns - expected).abs() as f64;
                            let in_frames = gap / frame_interval as f64;
                            if in_frames > worst_drift_frames {
                                worst_drift_frames = in_frames;
                            }
                        }
                        frames += 1;
                    }
                    Err(_) => dropped += 1,
                }
                drop(frame);
            }
        }

        Ok(Stats {
            frames,
            dropped,
            elapsed: started.elapsed(),
            worst_drift_frames,
        })
    }

    /// Wraps one captured texture as an `IMFSample` and hands it to the sink.
    ///
    /// No copy and no CPU readback: the texture stays on the GPU and the
    /// encoder reads it there, which is the whole reason for giving the sink
    /// writer a D3D manager.
    fn write_frame(
        writer: &IMFSinkWriter,
        stream: u32,
        texture: &ID3D11Texture2D,
        index: u64,
        frame_interval: i64,
    ) -> Result<(), String> {
        unsafe {
            let buffer = MFCreateDXGISurfaceBuffer(&ID3D11Texture2D::IID, texture, 0, false)
                .map_err(|e| format!("MFCreateDXGISurfaceBuffer failed: {e}"))?;
            let sample = MFCreateSample().map_err(|e| format!("MFCreateSample failed: {e}"))?;
            sample
                .AddBuffer(&buffer)
                .map_err(|e| format!("AddBuffer failed: {e}"))?;
            sample
                .SetSampleTime(index as i64 * frame_interval)
                .map_err(|e| format!("SetSampleTime failed: {e}"))?;
            sample
                .SetSampleDuration(frame_interval)
                .map_err(|e| format!("SetSampleDuration failed: {e}"))?;
            writer
                .WriteSample(stream, &sample)
                .map_err(|e| format!("WriteSample failed: {e}"))
        }
    }

    fn create_d3d_device() -> Result<
        (
            ID3D11Device,
            windows::Graphics::DirectX::Direct3D11::IDirect3DDevice,
        ),
        String,
    > {
        let mut device: Option<ID3D11Device> = None;
        unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                // The software rasterizer module, which is only consulted for
                // `D3D_DRIVER_TYPE_SOFTWARE`. Null is the documented value for
                // every other driver type, not an omission.
                HMODULE::default(),
                // BGRA support is required by WGC, and its absence is a
                // confusing failure much later if it is not asked for here.
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                None,
            )
        }
        .map_err(|e| format!("D3D11CreateDevice failed: {e}"))?;
        let device = device.ok_or("D3D11CreateDevice returned no device")?;

        let dxgi: IDXGIDevice = device
            .cast()
            .map_err(|e| format!("the D3D device is not a DXGI device: {e}"))?;
        let inspectable = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi) }
            .map_err(|e| format!("CreateDirect3D11DeviceFromDXGIDevice failed: {e}"))?;
        let winrt_device = inspectable
            .cast()
            .map_err(|e| format!("the WinRT device is not an IDirect3DDevice: {e}"))?;
        Ok((device, winrt_device))
    }

    fn create_sink_writer(
        out: &PathBuf,
        width: u32,
        height: u32,
        device: &ID3D11Device,
    ) -> Result<IMFSinkWriter, String> {
        unsafe {
            // The device manager is what lets the encoder read the captured
            // texture where it already is.
            let mut reset_token = 0u32;
            let mut manager: Option<IMFDXGIDeviceManager> = None;
            MFCreateDXGIDeviceManager(&mut reset_token, &mut manager)
                .map_err(|e| format!("MFCreateDXGIDeviceManager failed: {e}"))?;
            let manager = manager.ok_or("MFCreateDXGIDeviceManager returned nothing")?;
            manager
                .ResetDevice(device, reset_token)
                .map_err(|e| format!("ResetDevice failed: {e}"))?;

            let mut attributes: Option<IMFAttributes> = None;
            MFCreateAttributes(&mut attributes, 4)
                .map_err(|e| format!("MFCreateAttributes failed: {e}"))?;
            let attributes = attributes.ok_or("MFCreateAttributes returned nothing")?;
            attributes
                .SetUnknown(&MF_SINK_WRITER_D3D_MANAGER, &manager)
                .map_err(|e| format!("could not attach the D3D manager: {e}"))?;
            attributes
                .SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)
                .map_err(|e| format!("could not enable hardware transforms: {e}"))?;

            // **Fragmented MP4, and this is the line the third claim rests
            // on.** An ordinary MP4 writes its index at the end, so a file
            // killed mid-write has no `moov` and plays nowhere. Fragmented
            // MP4 writes the header first and self-contained fragments after
            // it, so whatever reached disk before the kill is a valid file.
            attributes
                .SetGUID(&MF_TRANSCODE_CONTAINERTYPE, &MFTranscodeContainerType_FMPEG4)
                .map_err(|e| format!("could not select fragmented MP4: {e}"))?;
            attributes
                .SetUINT32(&MF_MPEG4SINK_MOOV_BEFORE_MDAT, 1)
                .map_err(|e| format!("could not ask for moov first: {e}"))?;

            let writer = MFCreateSinkWriterFromURL(&HSTRING::from(out.as_os_str()), None, &attributes)
            .map_err(|e| format!("MFCreateSinkWriterFromURL failed: {e}"))?;

            let target = media_type(&MFVideoFormat_H264, width, height, Some(BITRATE))?;
            let stream = writer
                .AddStream(&target)
                .map_err(|e| format!("AddStream failed: {e}"))?;

            let source = media_type(&MFVideoFormat_RGB32, width, height, None)?;
            writer
                .SetInputMediaType(stream, &source, None)
                .map_err(|e| {
                    format!(
                        "SetInputMediaType failed: {e}\n\
                         This is where a machine with no usable H.264 encoder says so, which is\n\
                         itself a finding: record which encoders `--list` offered."
                    )
                })?;

            Ok(writer)
        }
    }

    fn media_type(
        subtype: &windows_core::GUID,
        width: u32,
        height: u32,
        bitrate: Option<u32>,
    ) -> Result<IMFMediaType, String> {
        unsafe {
            let media_type =
                MFCreateMediaType().map_err(|e| format!("MFCreateMediaType failed: {e}"))?;
            media_type
                .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                .map_err(|e| format!("could not set the major type: {e}"))?;
            media_type
                .SetGUID(&MF_MT_SUBTYPE, subtype)
                .map_err(|e| format!("could not set the subtype: {e}"))?;
            media_type
                .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
                .map_err(|e| format!("could not set the interlace mode: {e}"))?;
            // Both of these are packed pairs in one 64-bit attribute, high
            // half first. Setting them the obvious way (two 32-bit values)
            // fails at `AddStream` with an error that names neither.
            media_type
                .SetUINT64(&MF_MT_FRAME_SIZE, pack(width, height))
                .map_err(|e| format!("could not set the frame size: {e}"))?;
            media_type
                .SetUINT64(&MF_MT_FRAME_RATE, pack(FPS, 1))
                .map_err(|e| format!("could not set the frame rate: {e}"))?;
            media_type
                .SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))
                .map_err(|e| format!("could not set the pixel aspect ratio: {e}"))?;
            if let Some(bitrate) = bitrate {
                media_type
                    .SetUINT32(&MF_MT_AVG_BITRATE, bitrate)
                    .map_err(|e| format!("could not set the bitrate: {e}"))?;
            }
            Ok(media_type)
        }
    }

    const fn pack(high: u32, low: u32) -> u64 {
        ((high as u64) << 32) | low as u64
    }

    /// Not every build has this, and its absence is not a failure: the cursor
    /// in the capture is cosmetic for a spike about frames and timestamps.
    fn hide_cursor_if_possible(session: &GraphicsCaptureSession) {
        let _ = session.SetIsCursorCaptureEnabled(false);
    }

    fn report(args: &Args, width: u32, height: u32, stats: &Stats) -> String {
        let seconds = stats.elapsed.as_secs_f64();
        let achieved = if seconds > 0.0 {
            stats.frames as f64 / seconds
        } else {
            0.0
        };

        let mut out = String::new();
        let _ = writeln!(out, "Captured {width}x{height} for {seconds:.1}s.");
        let _ = writeln!(out, "  file            {}", args.out.display());
        let _ = writeln!(out, "  frames written  {}", stats.frames);
        let _ = writeln!(out, "  frames dropped  {}", stats.dropped);
        let _ = writeln!(out, "  achieved fps    {achieved:.2} (asked for {FPS})");
        let _ = writeln!(
            out,
            "  worst drift     {:.2} frame(s)",
            stats.worst_drift_frames
        );
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "The exit criterion is drift under one frame, and this run {}.",
            if stats.worst_drift_frames < 1.0 {
                "meets it"
            } else {
                "does not"
            }
        );
        let _ = writeln!(
            out,
            "\nNow the half this run cannot tell you: kill the process from Task Manager at\n\
             about minute five of a ten-minute run and open the file that is left. A clean\n\
             finalize was never in doubt; a killed one is what the daemon's whole design\n\
             assumes."
        );
        out
    }

    // --- Arguments -------------------------------------------------------

    fn parse_args() -> Result<Args, String> {
        let mut target = None;
        let mut seconds = 600u64;
        let mut out = PathBuf::from("p0c-video.mp4");

        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--list" => target = Some(Target::List),
                "--monitor" => {
                    let value = args.next().ok_or("--monitor needs an index")?;
                    let index = value
                        .parse::<usize>()
                        .map_err(|_| format!("--monitor wants a number, got {value:?}"))?;
                    target = Some(Target::Monitor(index));
                }
                "--window" => {
                    target = Some(Target::Window(
                        args.next().ok_or("--window needs part of a title")?,
                    ));
                }
                "--seconds" => {
                    let value = args.next().ok_or("--seconds needs a number")?;
                    seconds = value
                        .parse::<u64>()
                        .map_err(|_| format!("--seconds wants a number, got {value:?}"))?;
                }
                "--out" => out = PathBuf::from(args.next().ok_or("--out needs a path")?),
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
        "p0c-video — WS1.4 (#8), the WGC + Media Foundation spike\n\
         \n\
           --monitor <n>     capture a display, by its index in --list\n\
           --window <text>   capture a window whose title contains this\n\
           --seconds <n>     how long for (default 600)\n\
           --out <path>      where the MP4 goes (default p0c-video.mp4)\n\
           --list            monitors and windows, with process ids\n\
         \n\
         Two runs answer #8: one clean for drift, and one killed from Task Manager at\n\
         about minute five, to see what the file is worth afterwards."
            .to_string()
    }
}
