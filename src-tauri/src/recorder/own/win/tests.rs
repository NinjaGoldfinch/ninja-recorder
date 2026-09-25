//! The own backend against real Windows.
//!
//! What a hosted runner can run is the encoding half: a WARP device, a
//! synthetic frame and the software H.264 MFT, through the same sink writer
//! a recording uses. The runner image may not have Media Foundation (Windows
//! Server installs it as an optional feature), so that test **skips, and
//! says why**, rather than failing. Its report goes straight to stderr,
//! which the test harness does not capture, so a CI log shows which way it
//! went on every run.
//!
//! The video processor that scales a resized window into the recording's
//! size runs on the runner too, with a read-back of the pixels it wrote, on
//! WARP or the Basic Render Driver; it skips the same way if neither offers
//! a video processor.
//!
//! A WGC capture needs a desktop to composite a window on; that test is
//! `#[ignore]`d until it proves stable on the runner, and runs by hand with
//! `cargo test own_backend_records_a_window -- --ignored`.
//!
//! Process loopback needs an audio engine and a render endpoint, which a
//! runner does not have, so the game-audio track is tested from the feed
//! onwards with synthetic PCM, and the capture itself only by hand:
//! `cargo test process_loopback_activates -- --ignored` activates it on this
//! test's own process tree and reports what the stamps were. On a Windows 10
//! box that is the quickest check of whether the API exists there at all
//! (#237's floor test; `spikes/p0c-audio/README.md` has the full one).

use std::fs::File;
use std::io::Write as _;
use std::path::Path;

use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_TEXTURE2D_DESC,
    D3D11_USAGE_STAGING, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Media::MediaFoundation::{MFSTARTUP_FULL, MFShutdown, MFStartup};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};

use super::{audio, capture, device, encode, scale, session};
use crate::mp4;
use crate::recorder::own::fit::Size;
use crate::recorder::own::{clock, feed};

/// Straight to the process's stderr, past the harness's capture.
fn report(line: &str) {
    let _ = writeln!(std::io::stderr(), "[own backend test] {line}");
}

fn scratch_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("ninja-own-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

enum Outcome {
    Skipped(String),
    Written { encoder: String },
}

const W: u32 = 320;
const H: u32 = 240;
const TICKS: u64 = 120;

/// Fills `slot` with vertical colour bars, so the encoder has something that
/// is not flat to compress.
fn paint(device: &device::Device, slot: &capture::Slot) {
    let mut pixels = vec![0u8; (W * H * 4) as usize];
    for y in 0..H {
        for x in 0..W {
            let i = ((y * W + x) * 4) as usize;
            let bar = (x * 8 / W) as u8;
            pixels[i] = if bar & 1 != 0 { 255 } else { 0 }; // B
            pixels[i + 1] = if bar & 2 != 0 { 255 } else { 0 }; // G
            pixels[i + 2] = if bar & 4 != 0 { 255 } else { 0 }; // R
            pixels[i + 3] = 255;
        }
    }
    // SAFETY: the slot is a live default-usage texture of W x H BGRA on this
    // device, and `pixels` is exactly W * H * 4 bytes with that row pitch.
    unsafe {
        device.context.UpdateSubresource(
            &slot.texture,
            0,
            None,
            pixels.as_ptr().cast(),
            W * 4,
            0,
        )
    };
}

/// Two seconds of a 440 Hz tone as 10 ms packets on the QPC clock, from
/// tick 0: what the game audio thread would send, with the stamps it would
/// carry if the engine's were real.
fn tone_packets(origin: i64) -> Vec<feed::Packet> {
    let rate = i64::from(audio::SAMPLE_RATE);
    let seconds = (TICKS / u64::from(session::FPS)) as i64;
    (0..seconds * 100)
        .map(|p| {
            let first = p * rate / 100;
            let pcm: Vec<i16> = (first..first + rate / 100)
                .flat_map(|n| {
                    let v = (n as f64 * 440.0 * std::f64::consts::TAU / rate as f64).sin();
                    let s = (v * 8_000.0) as i16;
                    [s, s]
                })
                .collect();
            feed::Packet {
                hns: origin + first * clock::HNS_PER_SECOND / rate,
                frames: (rate / 100) as u32,
                discontinuity: false,
                pcm,
                clock: clock::AudioClock::Qpc,
            }
        })
        .collect()
}

/// The sink-writer path, fed `TICKS` ticks of one synthetic frame, and with
/// `with_audio` two seconds of tone through a [`feed::Feed`] into the AAC
/// stream, interleaved tick by tick as the session does it. Media Foundation
/// must already be started.
fn write_synthetic(out: &Path, with_audio: bool) -> Result<Outcome, String> {
    let encoders = match encode::h264_encoders() {
        Ok(encoders) => encoders,
        Err(e) => return Ok(Outcome::Skipped(format!("no encoder list: {e}"))),
    };
    if !encoders.iter().any(|e| !e.hardware) {
        let names: Vec<&str> = encoders.iter().map(|e| e.name.as_str()).collect();
        return Ok(Outcome::Skipped(format!(
            "no software H.264 MFT on this image (offered: {names:?})"
        )));
    }
    let device = device::create_warp_device()?;
    let slots = capture::create_slots(&device.device, W, H, 2)?;
    paint(&device, &slots[0]);

    let audio_rate = with_audio.then_some(audio::SAMPLE_RATE);
    let sink = encode::Sink::create(out, W, H, session::FPS, &device.device, false, audio_rate)?;
    sink.begin()?;
    let loaded = sink.loaded()?;
    let origin = 1_000 * clock::HNS_PER_SECOND;
    let mut feed = feed::Feed::new(audio::SAMPLE_RATE, origin);
    if with_audio {
        for packet in tone_packets(origin) {
            feed.push(packet);
        }
    }
    let mut write = |pcm: &[i16], position: u64| sink.write_audio(pcm, position);
    for k in 0..TICKS {
        let t = clock::tick_time(k, session::FPS);
        let d = clock::tick_time(k + 1, session::FPS) - t;
        sink.write(&slots[0], t, d).map_err(|e| format!("{e} (at tick {k})"))?;
        if with_audio {
            feed.write_ready(t + d, &mut write)?;
        }
    }
    if with_audio {
        feed.finish(clock::tick_time(TICKS, session::FPS), &mut write)?;
        let written = feed.aligner().written();
        if written != u64::from(audio::SAMPLE_RATE) * TICKS / u64::from(session::FPS) {
            return Err(format!("the feed wrote {written} audio frames"));
        }
    }
    sink.finalize()?;
    drop(slots);
    Ok(Outcome::Written {
        encoder: format!(
            "{} (hardware: {})",
            loaded.name.as_deref().unwrap_or("(no name)"),
            loaded.hardware()
        ),
    })
}

/// A WARP device, a synthetic BGRA texture and the software MFT write 120
/// ticks through the sink writer, and the file is a fragmented MP4 with
/// complete fragments and nothing but the video track.
#[test]
fn the_sink_writer_writes_a_fragmented_mp4_from_a_warp_device() {
    sink_writer_test("warp", false);
}

/// The same, with the Game preset's one AAC track fed synthetic PCM through
/// the feed the session uses (#237). Process loopback itself cannot run on
/// a runner, with no game and no audio engine; this is everything after it.
#[test]
fn the_sink_writer_writes_the_game_audio_track() {
    sink_writer_test("warp-aac", true);
}

fn sink_writer_test(name: &str, with_audio: bool) {
    let dir = scratch_dir(name);
    let out = dir.join(format!("{name}.mp4"));

    // SAFETY: once on this test's thread, before any COM use; paired below.
    let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.ok();
    assert!(com.is_ok(), "CoInitializeEx: {com:?}");

    let outcome = match device::media_foundation() {
        Err(e) => Ok(Outcome::Skipped(e)),
        // SAFETY: plain call; paired with MFShutdown below.
        Ok(()) => match unsafe { MFStartup(session::mf_version(), MFSTARTUP_FULL) } {
            Err(e) => Ok(Outcome::Skipped(format!("MFStartup failed: {e}"))),
            Ok(()) => {
                let outcome = write_synthetic(&out, with_audio);
                // SAFETY: pairs MFStartup; every MF object above is dropped.
                let _ = unsafe { MFShutdown() };
                outcome
            }
        },
    };
    // SAFETY: pairs CoInitializeEx; every COM object above is dropped.
    unsafe { CoUninitialize() };

    match outcome {
        Ok(Outcome::Skipped(why)) => {
            report(&format!("SKIPPED the WARP sink-writer test ({name}): {why}"));
        }
        Ok(Outcome::Written { encoder }) => {
            let mut file = File::open(&out).expect("the sink writer left no file");
            let summary = mp4::summarize(&mut file).expect("summarize");
            report(&format!(
                "RAN the WARP sink-writer test ({name}): {TICKS} ticks via {encoder}; {} bytes, \
                 layout {}",
                summary.file_len, summary.layout
            ));
            assert!(summary.structurally_playable(), "{summary:?}");
            assert!(summary.mvex, "not fragmented: {summary:?}");
            assert!(summary.complete_fragments > 0, "{summary:?}");
            assert_eq!(summary.truncated, None, "{summary:?}");
            let audio = u32::from(with_audio);
            assert_eq!(summary.tracks, 1 + audio, "video and {audio} audio: {summary:?}");
            assert_eq!(summary.audio_tracks, audio, "{summary:?}");
        }
        Err(e) => panic!("the sink-writer path failed: {e}"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

// --- The video processor, on the runner (#240) -----------------------------

/// Fills a `size` BGRA texture with one colour, `[b, g, r, a]`.
fn fill(device: &device::Device, texture: &ID3D11Texture2D, size: Size, bgra: [u8; 4]) {
    let pixels: Vec<u8> = bgra.repeat((size.width * size.height) as usize);
    // SAFETY: `texture` is a live default-usage BGRA texture of `size` on
    // this device, and `pixels` is exactly that many 4-byte pixels at that
    // row pitch.
    unsafe {
        device.context.UpdateSubresource(
            texture,
            0,
            None,
            pixels.as_ptr().cast(),
            size.width * 4,
            0,
        )
    };
}

/// Reads a `size` BGRA texture back to the CPU, tightly packed.
fn read_back(device: &device::Device, texture: &ID3D11Texture2D, size: Size) -> Vec<u8> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: size.width,
        Height: size.height,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        MiscFlags: 0,
    };
    let mut staging: Option<ID3D11Texture2D> = None;
    // SAFETY: `desc` is complete; no initial data.
    unsafe { device.device.CreateTexture2D(&desc, None, Some(&mut staging)) }
        .expect("staging texture");
    let staging = staging.expect("staging texture");
    // SAFETY: both live on this device, same size and format.
    unsafe { device.context.CopyResource(&staging, texture) };
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    // SAFETY: `staging` is CPU-readable; `mapped` is a live out-parameter.
    unsafe { device.context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)) }
        .expect("Map");
    let row = (size.width * 4) as usize;
    let mut out = Vec::with_capacity(row * size.height as usize);
    for y in 0..size.height as usize {
        // SAFETY: the mapping is `RowPitch` bytes a row for `size.height`
        // rows, each at least `row` bytes; it stays mapped until Unmap below.
        let line = unsafe {
            std::slice::from_raw_parts(
                mapped.pData.cast::<u8>().add(y * mapped.RowPitch as usize),
                row,
            )
        };
        out.extend_from_slice(line);
    }
    // SAFETY: mapped above, and nothing reads the mapping after this.
    unsafe { device.context.Unmap(&staging, 0) };
    out
}

fn pixel(pixels: &[u8], size: Size, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * size.width + x) * 4) as usize;
    [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
}

fn assert_near(got: [u8; 4], want: [u8; 4], what: &str) {
    let close = got.iter().zip(want).take(3).all(|(g, w)| g.abs_diff(w) <= 3);
    assert!(close, "{what}: got BGRA {got:?}, wanted about {want:?}");
}

const OUTPUT: Size = Size::new(1920, 1080);
const CONTENT: [u8; 4] = [40, 160, 220, 255];
const BLACK: [u8; 4] = [0, 0, 0, 255];
const WHITE: [u8; 4] = [255, 255, 255, 255];

/// One frame of `content` size through the fitter into a 1920x1080 slot that
/// starts white, so a bar that was not cleared shows as white rather than as
/// a texture's initial zeroes. Returns what the fitter did and the slot's
/// pixels.
fn fit_one(device: &device::Device, content: Size) -> Result<(scale::Placed, Vec<u8>), String> {
    let source = scale::create_bgra(device, content)?;
    fill(device, &source, content, CONTENT);
    let slots = capture::create_slots(&device.device, OUTPUT.width, OUTPUT.height, 1)?;
    fill(device, &slots[0].texture, OUTPUT, WHITE);
    let mut fitter = scale::Fitter::new(OUTPUT);
    let placed = fitter.place(device, &source, content, &slots[0].texture)?;
    Ok((placed, read_back(device, &slots[0].texture, OUTPUT)))
}

/// The video processor scales a 1280x720 frame to fill a 1920x1080 slot,
/// pillarboxes a 1280x1024 one with black bars, and a 1920x1080 frame is a
/// plain copy. Skips, and says why, where no device has a video processor.
#[test]
fn the_video_processor_letterboxes_a_resized_frame() {
    let (device, name) = match video_processor_device() {
        Ok(found) => found,
        Err(why) => {
            report(&format!("SKIPPED the video-processor test: {why}"));
            return;
        }
    };

    // The same aspect: scaled up to fill the frame, no bars.
    let (placed, px) = fit_one(&device, Size::new(1280, 720)).expect("1280x720");
    assert_eq!(placed, scale::Placed::Scaled);
    for (x, y) in [(2, 2), (1917, 2), (2, 1077), (1917, 1077), (960, 540)] {
        assert_near(pixel(&px, OUTPUT, x, y), CONTENT, &format!("1280x720 at ({x}, {y})"));
    }

    // 5:4: 1350x1080 at x = 284, black either side.
    let (placed, px) = fit_one(&device, Size::new(1280, 1024)).expect("1280x1024");
    assert_eq!(placed, scale::Placed::Scaled);
    for (x, y) in [(0, 0), (100, 540), (280, 1079), (1640, 0), (1820, 540), (1919, 1079)] {
        assert_near(pixel(&px, OUTPUT, x, y), BLACK, &format!("bar at ({x}, {y})"));
    }
    for (x, y) in [(290, 2), (960, 540), (1628, 1077)] {
        assert_near(pixel(&px, OUTPUT, x, y), CONTENT, &format!("1280x1024 at ({x}, {y})"));
    }

    // The recording's own size: copied, not scaled.
    let (placed, px) = fit_one(&device, OUTPUT).expect("1920x1080");
    assert_eq!(placed, scale::Placed::Copied);
    assert_near(pixel(&px, OUTPUT, 0, 0), CONTENT, "copied corner");
    assert_near(pixel(&px, OUTPUT, 1919, 1079), CONTENT, "copied corner");

    // What the session writes after the window closes.
    let slots = capture::create_slots(&device.device, OUTPUT.width, OUTPUT.height, 1)
        .expect("slot");
    fill(&device, &slots[0].texture, OUTPUT, WHITE);
    scale::fill_black(&device, &slots[0].texture).expect("fill_black");
    let px = read_back(&device, &slots[0].texture, OUTPUT);
    assert_near(pixel(&px, OUTPUT, 960, 540), BLACK, "after fill_black");

    report(&format!("RAN the video-processor test on {name}: scaled, letterboxed, copied, blacked"));
}

/// A device with a video processor that reads and writes BGRA: WARP first,
/// then each adapter DXGI lists. A GPU-less runner lists the Microsoft Basic
/// Render Driver, WARP behind a driver interface, which may offer the video
/// DDI where a WARP device made directly does not. The error names why each
/// candidate was refused.
fn video_processor_device() -> Result<(device::Device, String), String> {
    let mut candidates: Vec<(String, Result<device::Device, String>)> =
        vec![("WARP".to_string(), device::create_warp_device())];
    match device::adapters() {
        Ok(adapters) => candidates.extend(
            adapters.iter().map(|a| (a.info.name.clone(), device::create_device(&a.adapter))),
        ),
        Err(e) => candidates.push(("the adapter list".to_string(), Err(e))),
    }
    let mut refused = Vec::new();
    for (name, device) in candidates {
        let probe = device.and_then(|device| {
            scale::Processor::new(&device, Size::new(1280, 1024), OUTPUT, DXGI_FORMAT_B8G8R8A8_UNORM)
                .map(|_| device)
        });
        match probe {
            Ok(device) => return Ok((device, name)),
            Err(e) => refused.push(format!("{name}: {e}")),
        }
    }
    Err(refused.join("; "))
}

/// The whole backend against a window of the game's class: prepare, start,
/// two seconds, stop. Needs a desktop session for WGC, so it is ignored
/// until it proves stable on a runner.
#[test]
#[ignore = "needs a desktop session for WGC; run by hand on a Windows box"]
fn own_backend_records_a_window() {
    use crate::recorder::{RecordConfig, Recorder};
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, MSG, PM_REMOVE,
        PeekMessageW, RegisterClassW, SW_SHOW, ShowWindow, TranslateMessage, WNDCLASSW,
        WS_OVERLAPPEDWINDOW, WS_EX_LEFT,
    };
    use windows::core::{PCWSTR, w};

    extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        // SAFETY: forwarding the message the system just delivered, as is.
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }

    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop_flag = stop.clone();
    let pump = std::thread::spawn(move || {
        // SAFETY: a null module name is this process's own image.
        let instance = unsafe { GetModuleHandleW(PCWSTR::null()) }.expect("module handle");
        let class = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            lpszClassName: w!("RiotWindowClass"),
            ..Default::default()
        };
        // SAFETY: `class` is fully initialised and its strings are static.
        unsafe { RegisterClassW(&class) };
        // SAFETY: the class was registered above; no parent, menu or param.
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_LEFT,
                w!("RiotWindowClass"),
                w!("own backend test"),
                WS_OVERLAPPEDWINDOW,
                100,
                100,
                641,
                481,
                None,
                None,
                Some(instance.into()),
                None,
            )
        }
        .expect("CreateWindowExW");
        // SAFETY: `hwnd` is the window just created on this thread.
        let _ = unsafe { ShowWindow(hwnd, SW_SHOW) };
        let mut msg = MSG::default();
        while !stop_flag.load(std::sync::atomic::Ordering::Acquire) {
            // SAFETY: `msg` is a live out-parameter; this thread owns the window.
            while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
                // SAFETY: `msg` was just filled by PeekMessageW.
                let _ = unsafe { TranslateMessage(&msg) };
                // SAFETY: as above.
                unsafe { DispatchMessageW(&msg) };
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        // SAFETY: this thread created the window.
        let _ = unsafe { DestroyWindow(hwnd) };
    });

    let dir = scratch_dir("wgc");
    let mut recorder = super::OwnRecorder::new(None);
    recorder.prepare().expect("prepare");
    report(&format!("prepared: {}", recorder.backend_name()));
    let config =
        RecordConfig { output_dir: dir.clone(), file_stem: "wgc".into(), ..Default::default() };
    recorder.start(config).expect("start");
    std::thread::sleep(std::time::Duration::from_secs(2));
    let output = recorder.stop().expect("stop");
    report(&format!("recorded with {}", recorder.backend_name()));
    recorder.release();
    stop.store(true, std::sync::atomic::Ordering::Release);
    pump.join().expect("pump");

    let mut file = File::open(&output.path).expect("file");
    let summary = mp4::summarize(&mut file).expect("summarize");
    report(&format!("WGC run: {} bytes, layout {}", summary.file_len, summary.layout));
    assert!(summary.structurally_playable(), "{summary:?}");
    // The window is this test's, owned by no `League of Legends.exe`, so
    // there is no game audio to capture: video only, and reported as such.
    assert!(output.audio.tracks.is_empty());
    assert_eq!(summary.audio_tracks, 0, "{summary:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Process loopback activates and starts on this build of Windows, on this
/// test's own process tree. Nothing in the tree plays anything, so packets
/// may not come at all (a silent target is not guaranteed to be fed); what
/// passes is the activation, the format and the start, which is the part
/// that does not exist below the OS floor. Needs an audio engine and a
/// render endpoint, so it is ignored on the runner.
#[test]
#[ignore = "needs an audio engine and a render endpoint; run by hand on a Windows box"]
fn process_loopback_activates() {
    report(&format!("Windows build {:?}", device::windows_build()));
    let (tx, rx) = std::sync::mpsc::channel();
    // The source thread initialises its own COM; this one needs none.
    let outcome = audio::start_game(std::process::id(), tx).map(|source| {
        std::thread::sleep(std::time::Duration::from_secs(2));
        source.stop()
    });
    let received = rx.try_iter().count();
    match outcome {
        Ok(Ok(summary)) => report(&format!(
            "process loopback ran: {} packets ({received} received), clock {:?}",
            summary.packets, summary.clock
        )),
        Ok(Err(e)) => panic!("process loopback started, then failed: {e}"),
        Err(e) => panic!("process loopback did not start: {e}"),
    }
}
