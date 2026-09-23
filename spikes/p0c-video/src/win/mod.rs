//! Everything that needs Windows: the environment report, `--list`, the
//! capture loop, and the parent half of a `--kill-after` run.

mod audio;
mod encode;
mod video;

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::fs::File;
use std::io::Write as _;
use std::path::Path;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::channel;
use std::time::{Duration, Instant};

use windows::Foundation::TypedEventHandler;
use windows::Graphics::Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem};
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Wdk::System::SystemServices::RtlGetVersion;
use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;
use windows::Win32::Media::MediaFoundation::{MFSTARTUP_FULL, MFShutdown, MFStartup};
use windows::Win32::Media::{timeBeginPeriod, timeEndPeriod};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::System::SystemInformation::OSVERSIONINFOW;
use windows::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
use windows::Win32::System::WinRT::Direct3D11::IDirect3DDxgiInterfaceAccess;
use windows::core::{IInspectable, Interface};

use crate::clock::{self, Aligner, HNS_PER_SECOND};
use crate::verify::{self, Fed};
use crate::{Args, AudioSource, Encoder, FPS};

/// The exit code the child terminates itself with, so the parent can tell a
/// planned kill from a capture that failed on its own.
const KILL_EXIT_CODE: u32 = 0x4B11; // "KILL"

/// Textures frames are copied into. Enough that the encoder holding a few
/// never leaves the capture without somewhere to put the next one; running
/// out is counted.
const SLOTS: usize = 8;

/// How long to wait for WGC's first frame before calling the target dead.
/// WGC sends one on start for anything visible; a minimised window sends none.
const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(10);

pub fn run(args: &Args) -> Result<(), String> {
    if args.kill_after.is_some() && !args.child && !args.list {
        return run_parent(args);
    }

    // MTA for the duration: WGC's free-threaded pool, Media Foundation and
    // WASAPI all accept it, and this binary has no window to need an STA.
    // SAFETY: once, on this thread, before any COM use; paired below.
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
        .ok()
        .map_err(|e| format!("CoInitializeEx failed: {e}"))?;
    // SAFETY: plain call. MF needs it before any MFCreate*.
    let started = unsafe { MFStartup(mf_version(), MFSTARTUP_FULL) };
    // 1 ms scheduler resolution, so the cadence loop's sleeps land within a
    // millisecond of the tick they wait for instead of within 15.6.
    // SAFETY: plain call, paired with timeEndPeriod below.
    unsafe { timeBeginPeriod(1) };

    let result = match &started {
        Err(e) => Err(format!("MFStartup failed: {e}")),
        Ok(()) if args.list => list(),
        Ok(()) => record(args),
    };

    // SAFETY: pairs timeBeginPeriod(1).
    unsafe { timeEndPeriod(1) };
    if started.is_ok() {
        // SAFETY: pairs MFStartup; every MF object has been dropped.
        let _ = unsafe { MFShutdown() };
    }
    // SAFETY: pairs CoInitializeEx; every COM object has been dropped.
    unsafe { CoUninitialize() };
    result
}

/// `MF_VERSION`, which windows-rs does not expose as a constant:
/// `(MF_SDK_VERSION << 16) | MF_API_VERSION`, both fixed.
fn mf_version() -> u32 {
    const MF_SDK_VERSION: u32 = 0x0002;
    const MF_API_VERSION: u32 = 0x0070;
    (MF_SDK_VERSION << 16) | MF_API_VERSION
}

/// The performance counter in 100 ns units: the timebase WASAPI's packet
/// positions and WGC's `SystemRelativeTime` are both reported in, which is
/// what makes them comparable at all.
fn qpc_hns() -> i64 {
    static FREQUENCY: OnceLock<i64> = OnceLock::new();
    let frequency = *FREQUENCY.get_or_init(|| {
        let mut f = 0i64;
        // SAFETY: `f` is a live out-parameter; this cannot fail on XP or later.
        let _ = unsafe { QueryPerformanceFrequency(&mut f) };
        f.max(1)
    });
    let mut count = 0i64;
    // SAFETY: `count` is a live out-parameter.
    let _ = unsafe { QueryPerformanceCounter(&mut count) };
    (i128::from(count) * i128::from(HNS_PER_SECOND) / i128::from(frequency)) as i64
}

fn windows_build() -> String {
    let mut info = OSVERSIONINFOW {
        dwOSVersionInfoSize: size_of::<OSVERSIONINFOW>() as u32,
        ..Default::default()
    };
    // SAFETY: `info` is live with its size field set, which is the whole of
    // RtlGetVersion's contract. GetVersionEx would lie to an unmanifested exe.
    if unsafe { RtlGetVersion(&mut info) }.is_ok() {
        format!(
            "{}.{} build {}",
            info.dwMajorVersion, info.dwMinorVersion, info.dwBuildNumber
        )
    } else {
        "unknown".to_string()
    }
}

// --- --list --------------------------------------------------------------

fn print_adapters(adapters: &[video::Adapter]) {
    println!("GPUs (--adapter <n>):");
    for a in adapters {
        println!(
            "  {}  {}  vendor {:#06x} {}  device {:#06x}  {} MB{}",
            a.index,
            a.name,
            a.vendor,
            video::vendor_name(a.vendor),
            a.device,
            a.vram_mb,
            if a.software { "  (software)" } else { "" }
        );
    }
}

fn print_encoders(encoders: &[encode::EncoderInfo]) {
    println!("H.264 encoders Media Foundation offers, in its own order:");
    if encoders.is_empty() {
        println!("  (none)");
    }
    for e in encoders {
        let vendor = e
            .vendor
            .as_deref()
            .map(|v| {
                let name = encode::vendor_id(v).map_or("unparsed", video::vendor_name);
                format!("  {v} = {name}")
            })
            .unwrap_or_default();
        println!(
            "  [{}] {}{vendor}{}",
            if e.hardware { "hardware" } else { "software" },
            e.name,
            e.url
                .as_deref()
                .map(|u| format!("\n             {u}"))
                .unwrap_or_default()
        );
    }
}

fn list() -> Result<(), String> {
    println!("== p0c-video --list (WS1.4, #8) ==");
    println!("windows   {}", windows_build());
    println!();
    print_adapters(&video::adapters()?);
    println!();
    print_encoders(&encode::h264_encoders()?);
    println!();
    println!("Monitors (--monitor <n>):");
    for (i, m) in video::monitors().iter().enumerate() {
        let b = m.bounds;
        println!(
            "  {i}  {}x{} at ({}, {})",
            b.right - b.left,
            b.bottom - b.top,
            b.left,
            b.top
        );
    }
    println!();
    match video::game_window() {
        Some(w) => println!(
            "Game window (the default target): {:?}, pid {}",
            w.title, w.pid
        ),
        None => println!(
            "Game window (the default target): none. It exists from loading screen to end of game."
        ),
    }
    println!();
    println!("Visible windows (--window <part of the title>):");
    for w in video::visible_windows() {
        println!("  pid {:>6}  {}", w.pid, w.title);
    }
    println!();
    for source in [AudioSource::System, AudioSource::Mic] {
        let (tx, _rx) = channel();
        match audio::start(source, tx) {
            Ok(handle) => {
                let i = &handle.info;
                println!(
                    "Audio {:<8} {}: {} Hz, {} ch, {} ({})",
                    if source == AudioSource::System {
                        "system"
                    } else {
                        "mic"
                    },
                    i.source,
                    i.rate,
                    i.channels,
                    i.format.name(),
                    i.device_id
                );
                handle.stop()?;
            }
            Err(e) => println!("Audio {:?}: {e}", source),
        }
    }
    Ok(())
}

// --- The kill run --------------------------------------------------------

/// Run the capture as a child that terminates itself, then check the file.
///
/// `TerminateProcess` is the call Task Manager's End task makes: the process
/// stops between two instructions, with no destructor, no `Finalize` and no
/// flush of anything Media Foundation holds in memory. The child calls it on
/// itself at exactly `--kill-after` seconds, so the kill point is the same
/// on every attempt; `--verify` covers a kill from Task Manager by hand.
fn run_parent(args: &Args) -> Result<(), String> {
    let kill = args.kill_after.unwrap_or_default();
    println!("== p0c-video kill run (WS1.4, #8) ==");
    println!(
        "The capture runs as a child process and calls TerminateProcess on itself at {kill} s,\n\
         as Task Manager's End task would. This process then checks what was left on disk.\n"
    );
    let exe = std::env::current_exe().map_err(|e| format!("cannot find this executable: {e}"))?;
    let mut child_args: Vec<String> = std::env::args().skip(1).collect();
    child_args.push("--child".to_string());
    let status = std::process::Command::new(exe)
        .args(&child_args)
        .status()
        .map_err(|e| format!("could not start the capture child: {e}"))?;

    println!();
    let planned = status.code() == Some(KILL_EXIT_CODE as i32);
    println!(
        "child exited  {status}{}",
        if planned {
            " - terminated as planned"
        } else {
            " - NOT the planned kill: the capture ended some other way (see its output above)"
        }
    );
    println!();
    let findings = verify::check(
        &args.out,
        args.ffmpeg.as_deref(),
        args.audio != AudioSource::None,
    )?;
    print!("{}", verify::render(&args.out, &findings, FPS));
    println!();
    println!("== DEVELOPMENT.md §16, P0c-2 ==");
    let verdict = match findings.verdict() {
        verify::Verdict::Playable => "yes".to_string(),
        verify::Verdict::PlayableWithErrors => "yes, with decoder errors at the cut".to_string(),
        verify::Verdict::NotPlayable(why) => format!("NO - {why}"),
        verify::Verdict::Undecided => "undecided - no ffmpeg to decode it".to_string(),
    };
    let lost = match (&findings.fed, findings.video_frames) {
        (Some(fed), Some(frames)) => format!(
            "; {:.2} s of the {:.2} s fed to the sink was not on disk",
            (fed.video_ticks as f64 - frames as f64) / f64::from(FPS),
            fed.seconds
        ),
        _ => String::new(),
    };
    println!(
        "A file killed at minute five is playable : {verdict}{lost}{}",
        if planned {
            ""
        } else {
            " (the kill did not happen as planned)"
        }
    );
    Ok(())
}

// --- The capture ---------------------------------------------------------

#[derive(Default)]
struct VideoStats {
    /// Frames WGC delivered.
    arrivals: u64,
    /// Delivered frames replaced by a newer one before any tick used them.
    superseded: u64,
    ticks: u64,
    /// Ticks that repeated the previous frame because nothing new arrived.
    duplicates: u64,
    /// How late the loop wrote a tick, against the tick's own time.
    worst_late: i64,
    /// Tick time minus the capture time of the frame it showed.
    worst_age: i64,
    sum_age: i128,
    longest_gap: i64,
    resizes: u64,
    no_free_slot: u64,
    brightness: Vec<f64>,
}

struct Latest {
    slot: usize,
    /// Capture time relative to the origin.
    time: i64,
    fresh: bool,
}

fn frame_texture(
    frame: &windows::Graphics::Capture::Direct3D11CaptureFrame,
) -> Result<ID3D11Texture2D, String> {
    let surface = frame
        .Surface()
        .map_err(|e| format!("frame has no surface: {e}"))?;
    let access: IDirect3DDxgiInterfaceAccess = surface
        .cast()
        .map_err(|e| format!("surface is not a DXGI interface: {e}"))?;
    // SAFETY: `access` is live; the texture is a new reference we own.
    unsafe { access.GetInterface::<ID3D11Texture2D>() }
        .map_err(|e| format!("could not reach the texture: {e}"))
}

fn write_silence(sink: &encode::Sink, from: u64, frames: u64, rate: u32) -> Result<(), String> {
    let mut position = from;
    let mut left = frames;
    while left > 0 {
        let n = left.min(u64::from(rate));
        encode::write_audio(sink, &vec![0i16; n as usize * 2], position, rate)?;
        position += n;
        left -= n;
    }
    Ok(())
}

/// Place one packet and write what the aligner decided.
fn write_packet(
    sink: &encode::Sink,
    aligner: &mut Aligner,
    packet: &audio::Packet,
    origin: i64,
) -> Result<(), String> {
    let before = aligner.written();
    let rate = aligner.rate();
    let placement = aligner.place(packet.frames, packet.qpc_hns - origin, packet.discontinuity);
    write_silence(sink, before, placement.silence, rate)?;
    let skip = placement.skip.min(packet.frames) as usize;
    let mut pcm = Vec::with_capacity((packet.frames as usize + placement.repeat as usize) * 2);
    if skip < packet.frames as usize {
        for _ in 0..placement.repeat {
            pcm.extend_from_slice(&packet.pcm[skip * 2..skip * 2 + 2]);
        }
        pcm.extend_from_slice(&packet.pcm[skip * 2..]);
    }
    encode::write_audio(sink, &pcm, before + placement.silence, rate)
}

struct Progress {
    file: Option<File>,
}

impl Progress {
    fn create(out: &Path) -> Progress {
        Progress {
            file: File::create(verify::progress_path(out)).ok(),
        }
    }

    fn write(&mut self, fed: &Fed) {
        if let Some(file) = &mut self.file {
            // One line per second, appended. No fsync: a killed process's
            // completed writes are in the OS cache and survive it; only a
            // power cut would lose them, and that is not what is being tested.
            let _ = writeln!(file, "{}", verify::format_fed(fed));
        }
    }
}

fn record(args: &Args) -> Result<(), String> {
    println!("== p0c-video (WS1.4, #8) ==");
    println!("windows       {}", windows_build());
    println!(
        "mode          {}",
        if args.child {
            "kill run (child)"
        } else {
            "clean run"
        }
    );

    let adapters = video::adapters()?;
    let adapter = adapters
        .iter()
        .find(|a| a.index == args.adapter)
        .ok_or_else(|| {
            format!(
                "no adapter {}; --list shows {}",
                args.adapter,
                adapters.len()
            )
        })?;
    println!(
        "adapter       {}: {} ({:#06x}, {})",
        adapter.index,
        adapter.name,
        adapter.vendor,
        video::vendor_name(adapter.vendor)
    );

    let encoders = encode::h264_encoders()?;
    let offered: Vec<String> = encoders
        .iter()
        .map(|e| {
            format!(
                "{}{}",
                e.name,
                e.vendor
                    .as_deref()
                    .map(|v| format!(" [{v}]"))
                    .unwrap_or_default()
            )
        })
        .collect();
    println!(
        "offered       {}",
        if offered.is_empty() {
            "(none)".to_string()
        } else {
            offered.join("; ")
        }
    );
    if args.encoder == Encoder::Hardware {
        // The plan's vendor-ID check: refuse on none, rather than let the sink
        // writer fall back to software without saying so.
        let matching = encoders.iter().any(|e| {
            e.hardware && e.vendor.as_deref().and_then(encode::vendor_id) == Some(adapter.vendor)
        });
        if !matching {
            return Err(format!(
                "refusing: no hardware H.264 encoder from {} on adapter {}. That is the plan's \
                 \"refuse on none\", and it is a finding: record the 'offered' line above. \
                 --encoder software runs #68's second arm; --adapter picks another GPU.",
                video::vendor_name(adapter.vendor),
                adapter.index
            ));
        }
    }

    let device = video::create_device(&adapter.adapter)?;
    let resolved = video::capture_item(&args.target)?;
    let item_size = resolved
        .item
        .Size()
        .map_err(|e| format!("the capture item has no size: {e}"))?;
    // H.264 wants even dimensions and a window can be any size.
    let width = (item_size.Width.max(0) as u32) & !1;
    let height = (item_size.Height.max(0) as u32) & !1;
    if width == 0 || height == 0 {
        return Err("the capture target has no area (minimised?)".to_string());
    }
    println!("target        {}, {width}x{height}", resolved.description);

    let (audio_tx, audio_rx) = channel();
    let audio = match args.audio {
        AudioSource::None => None,
        source => Some(audio::start(source, audio_tx)?),
    };
    if let Some(a) = &audio {
        let i = &a.info;
        println!(
            "audio         {}: {} Hz, {} ch, {}{} ({})",
            i.source,
            i.rate,
            i.channels,
            i.format.name(),
            if i.keep_alive {
                ", with a silent keep-alive stream"
            } else {
                ""
            },
            i.device_id
        );
        println!("audio clock   {}", args.audio_clock.name());
    } else {
        println!("audio         none (--audio none): no A/V drift can be measured");
    }

    let sink = encode::create_sink(&encode::SinkConfig {
        out: &args.out,
        width,
        height,
        fps: FPS,
        device: &device.device,
        encoder: args.encoder,
        audio_rate: audio.as_ref().map(|a| a.info.rate),
    })?;
    // SAFETY: every stream has its input type set.
    unsafe { sink.writer.BeginWriting() }.map_err(|e| format!("BeginWriting failed: {e}"))?;

    let chosen = encode::chosen_transforms(&sink.writer, sink.video)?;
    let encoder_line = format!(
        "{}{}",
        chosen
            .encoder_name
            .as_deref()
            .unwrap_or("(the encoder reports no name)"),
        chosen
            .encoder_vendor
            .as_deref()
            .map(|v| format!(" [{v}]"))
            .unwrap_or_default()
    );
    println!("loaded        {encoder_line}");
    println!("chain         {}", chosen.chain.join(" -> "));
    let vendor_matches =
        chosen.encoder_vendor.as_deref().and_then(encode::vendor_id) == Some(adapter.vendor);
    if args.encoder == Encoder::Hardware && !chosen.hardware() {
        println!(
            "WARNING       the loaded encoder carries no hardware vendor id or URL. Either the sink\n\
             \x20             writer fell back to software without saying so, or this MFT does not\n\
             \x20             expose its attributes. Compare 'loaded' with --list before concluding."
        );
    }
    println!(
        "output        {} (fragmented MP4, H.264 {} Mbps{})",
        args.out.display(),
        encode::VIDEO_BITRATE / 1_000_000,
        if sink.audio.is_some() {
            " + AAC 192 kbps"
        } else {
            ""
        }
    );
    println!();

    let slots = video::create_slots(&device.device, width, height, SLOTS)?;
    let staging = video::create_texture(&device.device, &video::texture_desc(width, height, true))?;

    let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
        &device.winrt,
        DirectXPixelFormat::B8G8R8A8UIntNormalized,
        2,
        item_size,
    )
    .map_err(|e| format!("could not create the frame pool: {e}"))?;
    let mut pool_size = item_size;
    let closed = Arc::new(AtomicBool::new(false));
    let closed_flag = closed.clone();
    let closed_token = resolved
        .item
        .Closed(
            &TypedEventHandler::<GraphicsCaptureItem, IInspectable>::new(move |_, _| {
                closed_flag.store(true, Ordering::Release);
                Ok(())
            }),
        )
        .map_err(|e| format!("could not watch for the target closing: {e}"))?;
    let session = pool
        .CreateCaptureSession(&resolved.item)
        .map_err(|e| format!("could not create the capture session: {e}"))?;
    // The yellow border is the system's own "something is capturing this"
    // affordance. Leave it on: a capture that hides itself is what the
    // no-injection rule exists to avoid resembling.
    let _ = session.SetIsBorderRequired(true);
    let _ = session.SetIsCursorCaptureEnabled(false);
    session
        .StartCapture()
        .map_err(|e| format!("StartCapture failed: {e}"))?;

    // The first frame fixes the origin: tick 0 shows it, at its own capture
    // time, and every audio packet is placed relative to the same instant.
    let waited = Instant::now();
    let first = loop {
        if let Ok(frame) = pool.TryGetNextFrame() {
            break frame;
        }
        if waited.elapsed() > FIRST_FRAME_TIMEOUT {
            return Err(format!(
                "no frame from WGC in {} s. A minimised window produces none; exclusive \
                 fullscreen may produce none. Record the League window mode.",
                FIRST_FRAME_TIMEOUT.as_secs()
            ));
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let origin = first
        .SystemRelativeTime()
        .map_err(|e| format!("the first frame has no timestamp: {e}"))?
        .Duration;
    let first_texture = frame_texture(&first)?;
    let content = first.ContentSize().unwrap_or(item_size);
    video::copy_into(
        &device.context,
        &slots[0],
        &first_texture,
        width.min(content.Width as u32),
        height.min(content.Height as u32),
    );
    drop(first);

    let mut stats = VideoStats {
        arrivals: 1,
        ..VideoStats::default()
    };
    let mut latest = Latest {
        slot: 0,
        time: 0,
        fresh: true,
    };
    let mut next_slot = 1usize;
    let mut last_arrival = 0i64;
    let mut aligner = audio
        .as_ref()
        .map(|a| Aligner::new(a.info.rate, args.audio_clock));
    let mut pending: VecDeque<audio::Packet> = VecDeque::new();
    let mut progress = Progress::create(&args.out);
    let total_ticks = args.seconds * u64::from(FPS);
    let kill_at = args
        .kill_after
        .filter(|_| args.child)
        .map(|s| s as i64 * HNS_PER_SECOND);
    let mut k: u64 = 0;
    let mut next_report_second = 1i64;
    let started = Instant::now();

    println!(
        "     t   ticks  new-frames  dup%   late(f)  age(f) | raw drift ms (f)   residual ms  slips -/+   gaps"
    );
    let loop_result: Result<(), String> = (|| {
        while k < total_ticks && !closed.load(Ordering::Acquire) {
            // Everything WGC has, keeping only the newest.
            let mut newest = None;
            while let Ok(frame) = pool.TryGetNextFrame() {
                stats.arrivals += 1;
                if newest.is_some() {
                    stats.superseded += 1;
                }
                newest = Some(frame);
            }
            if let Some(frame) = newest {
                let time = frame
                    .SystemRelativeTime()
                    .map(|t| t.Duration - origin)
                    .unwrap_or(last_arrival);
                stats.longest_gap = stats.longest_gap.max(time - last_arrival);
                last_arrival = time;
                let content = frame.ContentSize().unwrap_or(pool_size);
                if content.Width != pool_size.Width || content.Height != pool_size.Height {
                    stats.resizes += 1;
                    pool.Recreate(
                        &device.winrt,
                        DirectXPixelFormat::B8G8R8A8UIntNormalized,
                        2,
                        content,
                    )
                    .map_err(|e| format!("could not resize the frame pool: {e}"))?;
                    pool_size = content;
                }
                let texture = frame_texture(&frame)?;
                let free = (0..slots.len())
                    .map(|o| (next_slot + o) % slots.len())
                    .find(|&i| i != latest.slot && !slots[i].busy());
                match free {
                    Some(i) => {
                        let w = width.min(content.Width.max(0) as u32);
                        let h = height.min(content.Height.max(0) as u32);
                        video::copy_into(&device.context, &slots[i], &texture, w, h);
                        // A frame no tick showed is replaced unseen.
                        if latest.fresh {
                            stats.superseded += 1;
                        }
                        latest = Latest {
                            slot: i,
                            time,
                            fresh: true,
                        };
                        next_slot = i + 1;
                    }
                    None => stats.no_free_slot += 1,
                }
            }

            // Every tick that is due, on the 60 fps grid.
            let now = qpc_hns() - origin;
            if let Some(due) = clock::ticks_due(now, FPS) {
                while k <= due && k < total_ticks {
                    let t = clock::tick_time(k, FPS);
                    let d = clock::tick_time(k + 1, FPS) - t;
                    encode::write_video(&sink, &slots[latest.slot], t, d)
                        .map_err(|e| format!("{e} (at tick {k})"))?;
                    stats.ticks += 1;
                    if !latest.fresh {
                        stats.duplicates += 1;
                    }
                    latest.fresh = false;
                    stats.worst_late = stats.worst_late.max(now - t);
                    let age = t - latest.time;
                    if age.abs() > stats.worst_age.abs() {
                        stats.worst_age = age;
                    }
                    stats.sum_age += i128::from(age);
                    k += 1;
                }
            }

            // Audio that has arrived, but only as far as the video has been
            // written: a packet that runs past the last tick waits, so the
            // audio track can never end after the video one.
            if let Some(aligner) = aligner.as_mut() {
                pending.extend(audio_rx.try_iter());
                let video_end = clock::tick_time(k, FPS);
                while let Some(packet) = pending.front() {
                    let rel = packet.qpc_hns - origin;
                    let packet_end =
                        clock::samples_at(rel, aligner.rate()) + i64::from(packet.frames);
                    if packet_end > clock::samples_at(video_end, aligner.rate()) {
                        break;
                    }
                    if let Some(packet) = pending.pop_front() {
                        write_packet(&sink, aligner, &packet, origin)?;
                    }
                }
            }

            let second = now / HNS_PER_SECOND;
            if second >= next_report_second {
                next_report_second = second + 1;
                progress.write(&fed(k, aligner.as_ref()));
                if second % 10 == 0 {
                    print_status(second, &stats, aligner.as_ref());
                }
                // Once early, then once a minute: a readback stalls the GPU
                // for a moment, and a black capture shows up at the first.
                if second == 5 || second % 60 == 0 {
                    stats.brightness.extend(video::brightness(
                        &device.context,
                        &staging,
                        &slots[latest.slot].texture,
                        width,
                        height,
                    ));
                }
            }

            if let Some(kill_at) = kill_at
                && now >= kill_at
            {
                progress.write(&fed(k, aligner.as_ref()));
                println!(
                    "TERMINATING  at {:.3} s, after {k} ticks, with no finalize: TerminateProcess on this process.",
                    now as f64 / HNS_PER_SECOND as f64
                );
                let _ = std::io::stdout().flush();
                // SAFETY: GetCurrentProcess is a pseudo-handle; terminating
                // ourselves is the point, and nothing after this line runs.
                let _ = unsafe { TerminateProcess(GetCurrentProcess(), KILL_EXIT_CODE) };
            }

            std::thread::sleep(Duration::from_millis(1));
        }
        Ok(())
    })();

    // Finishing: the audio up to the last tick, then padded to it exactly, so
    // that any A/V offset the file check finds was added downstream of here.
    let end = clock::tick_time(k, FPS);
    std::thread::sleep(Duration::from_millis(200));
    let audio_stop = audio.map(audio::Handle::stop);
    let mut pad = (0u64, 0u64);
    if let Some(aligner) = aligner.as_mut() {
        pending.extend(audio_rx.try_iter());
        drain_until(&sink, aligner, pending, origin, end)?;
        let before = aligner.written();
        pad = aligner.finish(end);
        write_silence(&sink, before, pad.0, aligner.rate())?;
    }
    let _ = session.Close();
    let _ = resolved.item.RemoveClosed(closed_token);
    let _ = pool.Close();
    let target_closed = closed.load(Ordering::Acquire);
    loop_result?;
    // SAFETY: writing has begun; every sample has been handed over.
    unsafe { sink.writer.Finalize() }.map_err(|e| format!("Finalize failed: {e}"))?;
    progress.write(&fed(k, aligner.as_ref()));
    if let Some(Err(e)) = audio_stop {
        println!("audio thread  ended with an error: {e}");
    }
    drop(sink);
    drop(slots);

    let wall = started.elapsed();
    let report = Report {
        args,
        encoder_line: &encoder_line,
        chain: chosen.chain.join(" -> "),
        vendor_matches,
        hardware: chosen.hardware(),
        offered: &offered,
        stats: &stats,
        aligner: aligner.as_ref(),
        pad,
        ticks: k,
        wall,
        target_closed,
    };
    print!("{}", report.render());

    if args.no_verify {
        return Ok(());
    }
    println!();
    let findings = verify::check(
        &args.out,
        args.ffmpeg.as_deref(),
        args.audio != AudioSource::None,
    )?;
    print!("{}", verify::render(&args.out, &findings, FPS));
    println!();
    print!("{}", report.rows(&findings));
    Ok(())
}

/// Write every queued packet that starts before `end`, truncating the one
/// that straddles it, and drop the rest.
fn drain_until(
    sink: &encode::Sink,
    aligner: &mut Aligner,
    pending: VecDeque<audio::Packet>,
    origin: i64,
    end: i64,
) -> Result<(), String> {
    let rate = aligner.rate();
    for mut packet in pending {
        let rel = packet.qpc_hns - origin;
        if rel >= end {
            continue;
        }
        let room = clock::samples_at(end, rate) - clock::samples_at(rel, rate);
        if room <= 0 {
            continue;
        }
        if room < i64::from(packet.frames) {
            packet.frames = room as u32;
            packet.pcm.truncate(packet.frames as usize * 2);
        }
        write_packet(sink, aligner, &packet, origin)?;
    }
    Ok(())
}

fn fed(ticks: u64, aligner: Option<&Aligner>) -> Fed {
    Fed {
        seconds: clock::tick_time(ticks, FPS) as f64 / HNS_PER_SECOND as f64,
        video_ticks: ticks,
        audio_samples: aligner.map_or(0, Aligner::written),
        audio_rate: aligner.map_or(0, Aligner::rate),
        fps: FPS,
    }
}

fn ms_and_frames(samples: i64, rate: u32) -> String {
    format!(
        "{:+.2} ms ({:+.3} f)",
        clock::samples_to_ms(samples, rate),
        clock::samples_to_frames(samples, rate, FPS)
    )
}

fn print_status(second: i64, s: &VideoStats, aligner: Option<&Aligner>) {
    let dup = if s.ticks > 0 {
        s.duplicates as f64 * 100.0 / s.ticks as f64
    } else {
        0.0
    };
    let audio = match aligner {
        Some(a) => {
            let st = &a.stats;
            format!(
                "{:>18}  {:>+10.2}  {:>5}/{:<5} {:>4}",
                ms_and_frames(st.raw_drift_last, a.rate()),
                clock::samples_to_ms(st.residual_last, a.rate()),
                st.slips_dropped,
                st.slips_repeated,
                st.gaps
            )
        }
        None => "(no audio)".to_string(),
    };
    println!(
        "{:>5}s  {:>6}  {:>10}  {:>4.1}  {:>7.2}  {:>6.2} | {audio}",
        second,
        s.ticks,
        s.arrivals,
        dup,
        clock::hns_to_frames(s.worst_late, FPS),
        clock::hns_to_frames(s.worst_age, FPS),
    );
}

struct Report<'a> {
    args: &'a Args,
    encoder_line: &'a str,
    chain: String,
    vendor_matches: bool,
    hardware: bool,
    offered: &'a [String],
    stats: &'a VideoStats,
    aligner: Option<&'a Aligner>,
    pad: (u64, u64),
    ticks: u64,
    wall: Duration,
    target_closed: bool,
}

impl Report<'_> {
    fn render(&self) -> String {
        let s = self.stats;
        let mut out = String::new();
        let seconds = self.ticks as f64 / f64::from(FPS);
        let _ = writeln!(out);
        let _ = writeln!(out, "== result ==");
        let _ = writeln!(
            out,
            "length        {seconds:.2} s on the grid ({} ticks), {:.2} s wall{}",
            self.ticks,
            self.wall.as_secs_f64(),
            if self.target_closed {
                "; the target CLOSED before the end"
            } else {
                ""
            }
        );
        let _ = writeln!(
            out,
            "WGC           {} frames delivered ({:.2} fps), {} superseded before a tick used them",
            s.arrivals,
            s.arrivals as f64 / seconds.max(1e-9),
            s.superseded
        );
        let _ = writeln!(
            out,
            "grid          {} ticks written, {} repeated the previous frame ({:.1}%)",
            s.ticks,
            s.duplicates,
            s.duplicates as f64 * 100.0 / (s.ticks.max(1)) as f64
        );
        let _ = writeln!(
            out,
            "video timing  worst tick written {:.2} frame(s) late; frame shown at a tick was at worst \
             {:+.2} frame(s) old, {:+.2} on average; longest WGC silence {:.2} frame(s)",
            clock::hns_to_frames(s.worst_late, FPS),
            clock::hns_to_frames(s.worst_age, FPS),
            clock::hns_to_frames((s.sum_age / i128::from(s.ticks.max(1) as i64)) as i64, FPS),
            clock::hns_to_frames(s.longest_gap, FPS),
        );
        let _ = writeln!(
            out,
            "              {} resize(s) of the target; {} frame(s) dropped for want of a free texture",
            s.resizes, s.no_free_slot
        );
        if !s.brightness.is_empty() {
            let min = s.brightness.iter().copied().fold(f64::INFINITY, f64::min);
            let mean = s.brightness.iter().sum::<f64>() / s.brightness.len() as f64;
            let _ = writeln!(
                out,
                "content       brightness 0-255 sampled at 5 s and once a minute: min {min:.1}, mean {mean:.1}{}",
                if min < 4.0 {
                    " - a near-black sample: check the file shows the game"
                } else {
                    ""
                }
            );
        }
        match self.aligner {
            Some(a) => {
                let st = &a.stats;
                let rate = a.rate();
                let _ = writeln!(
                    out,
                    "audio         {} packets, {} samples at {rate} Hz; {} discontinuit(y/ies) flagged by WASAPI",
                    st.packets, st.received, st.discontinuities
                );
                let _ = writeln!(
                    out,
                    "raw drift     device clock minus QPC: {} at the end, worst {}, {}",
                    ms_and_frames(st.raw_drift_last, rate),
                    ms_and_frames(st.raw_drift_worst, rate),
                    a.raw_ppm()
                        .map_or("-".to_string(), |p| format!("{p:+.1} ppm"))
                );
                let _ = writeln!(
                    out,
                    "correction    clock {}: {} sample(s) dropped, {} repeated; {} gap(s) filled ({} samples), \
                     {} overlap(s) cut ({} samples)",
                    self.args.audio_clock.name(),
                    st.slips_dropped,
                    st.slips_repeated,
                    st.gaps,
                    st.gap_samples,
                    st.overlaps,
                    st.overlap_samples
                );
                let _ = writeln!(
                    out,
                    "in the file   A/V residual {} at the end, worst {}; end padded with {} sample(s), \
                     overhang {}",
                    ms_and_frames(st.residual_last, rate),
                    ms_and_frames(st.residual_worst, rate),
                    self.pad.0,
                    self.pad.1
                );
            }
            None => {
                let _ = writeln!(out, "audio         none");
            }
        }
        let _ = writeln!(
            out,
            "encoder       {} ({}; vendor {} the adapter)",
            self.encoder_line,
            if self.hardware {
                "hardware"
            } else {
                "no hardware id"
            },
            if self.vendor_matches {
                "matches"
            } else {
                "does NOT match"
            }
        );
        let _ = writeln!(out, "chain         {}", self.chain);
        out
    }

    /// The four §16 rows, filled from this run and its file check.
    fn rows(&self, file: &verify::Findings) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "== DEVELOPMENT.md §16, P0c-2 rows ==");
        let reach = match (file.summary.structurally_playable(), file.video_frames) {
            (true, Some(n)) if n > 0 => format!(
                "yes: {n} frames decoded from {} complete fragment(s)",
                file.summary.complete_fragments
            ),
            (true, None) => "structure yes; not decoded (no ffmpeg)".to_string(),
            (false, _) => "NO: not a fragmented MP4 with a complete fragment".to_string(),
            (true, Some(_)) => "NO: nothing decoded".to_string(),
        };
        let _ = writeln!(out, "WGC frames reach a fragmented MP4        : {reach}");

        let drift = match self.aligner {
            Some(a) => {
                let st = &a.stats;
                let rate = a.rate();
                let residual = clock::samples_to_frames(st.residual_worst, rate, FPS);
                let raw = clock::samples_to_frames(st.raw_drift_worst, rate, FPS);
                let file_offset = file
                    .av_end_offset_us()
                    .map(|us| format!("{:+.2} f", us as f64 * f64::from(FPS) / 1e6))
                    .unwrap_or_else(|| "-".to_string());
                format!(
                    "{:.3} f written ({} clock), {:.3} f raw device drift ({}), file end offset {file_offset}; \
                     over {:.0} s. Written {} one frame.",
                    residual.abs(),
                    self.args.audio_clock.name(),
                    raw.abs(),
                    a.raw_ppm()
                        .map_or("-".to_string(), |p| format!("{p:+.1} ppm")),
                    self.ticks as f64 / f64::from(FPS),
                    if residual.abs() < 1.0 {
                        "is under"
                    } else {
                        "is NOT under"
                    }
                )
            }
            None => "not measured: run with audio".to_string(),
        };
        let _ = writeln!(out, "Worst drift over ten minutes, in frames  : {drift}");
        let _ = writeln!(
            out,
            "A file killed at minute five is playable : not this run (--kill-after 300 is the kill run)"
        );
        let _ = writeln!(
            out,
            "Encoder selected, and what was offered   : {} ({}, {} encode, vendor {} the adapter); offered: {}",
            self.encoder_line,
            if self.hardware {
                "hardware"
            } else {
                "no hardware id"
            },
            match self.args.encoder {
                Encoder::Hardware => "--encoder hardware",
                Encoder::Software => "--encoder software",
            },
            if self.vendor_matches {
                "matches"
            } else {
                "does not match"
            },
            self.offered.join("; ")
        );
        out
    }
}
