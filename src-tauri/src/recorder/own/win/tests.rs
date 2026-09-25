//! The own backend against real Windows.
//!
//! What a hosted runner can run is the encoding half: a WARP device, a
//! synthetic frame and **the software H.264 MFT, synchronous**, plus one AAC
//! MFT per audio track, through the same `output::Output` a recording uses,
//! into our own MP4 writer (#239). The runner image may not have Media
//! Foundation (Windows Server installs it as an optional feature), so those
//! tests **skip, and say why**, rather than failing. Their report goes
//! straight to stderr, which the test harness does not capture, so a CI log
//! shows which way each went on every run.
//!
//! **Frames reach the software encoder as NV12 in system memory, converted
//! on the CPU.** The runner has no video processor (neither WARP nor its
//! Basic Render Driver offers the D3D11 video DDI), so the GPU conversion a
//! recording would do cannot run there; `convert.rs` falls back to reading
//! the BGRA slot back and converting it with `own::nv12`, which is the path a
//! GPU-less machine really takes. The GPU conversion is covered by the video
//! processor test below, where a device has one, and by a recording.
//!
//! **The asynchronous (hardware) path cannot run on a runner**, which has no
//! GPU encoder: `hardware_encoder_writes_every_track` is `#[ignore]`d and runs
//! on a box with `cargo test hardware_encoder_writes_every_track -- --ignored`.
//!
//! Frames into slots (#240) are checked by reading the pixels back. The
//! copy, the crop-over-black fallback and `fill_black` run on WARP. The video
//! processor's scaling needs a device with the D3D11 video DDI, which neither
//! WARP nor the runner's Basic Render Driver offers, so that test skips there
//! and says so, and runs on a box with a GPU.
//!
//! A WGC capture needs a desktop to composite a window on; that test is
//! `#[ignore]`d until it proves stable on the runner, and runs by hand with
//! `cargo test own_backend_records_a_window -- --ignored`.
//!
//! Process loopback and the endpoints need an audio engine and devices, which
//! a runner does not have, so the audio tracks are tested from the mixer
//! onwards with synthetic PCM, one tone per source, through the same
//! `mix::TrackMix` a recording uses: the Game preset (one track), Game + mic
//! (the mix and two stems: a four-track file) and Game + mic + Discord (the
//! mix and three stems), with one source joining late and stopping early.
//! The captures themselves run only by hand:
//! `cargo test process_loopback_activates -- --ignored` activates process
//! loopback on this test's own process tree and reports what the stamps were
//! (on a Windows 10 box, the quickest check of whether the API exists there
//! at all: #237's floor test, and `spikes/p0c-audio/README.md` has the full
//! one), and `cargo test endpoints_capture -- --ignored` opens the default
//! microphone and the desktop in loopback.

use std::fs::File;
use std::io::Write as _;
use std::path::Path;
use std::time::{Duration, Instant};

use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_TEXTURE2D_DESC,
    D3D11_USAGE_STAGING, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Media::MediaFoundation::{MFSTARTUP_FULL, MFShutdown, MFStartup};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};

use super::{audio, capture, device, encode, output, scale, session};
use crate::mp4;
use crate::recorder::audio::{AudioLayout, AudioPreset};
use crate::recorder::own::fit::Size;
use crate::recorder::own::select::{self, Choice};
use crate::recorder::own::{clock, feed, mix};

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
    Written { encoder: String, how: String, tracks: String },
}

const W: u32 = 320;
const H: u32 = 240;
/// Five seconds: keyframes at 0, 2 and 4 s with a GOP of 120, so three
/// fragments, each opening on its keyframe.
const TICKS: u64 = 300;

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

/// 10 ms packets of a `hz` tone at `amplitude`, on the QPC clock, for
/// packets `from..to` counted from tick 0: what a source thread would send,
/// with the stamps it would carry if the engine's were real.
fn tone_packets(origin: i64, hz: f64, amplitude: f64, from: i64, to: i64) -> Vec<feed::Packet> {
    let rate = i64::from(audio::SAMPLE_RATE);
    (from..to)
        .map(|p| {
            let first = p * rate / 100;
            let pcm: Vec<f32> = (first..first + rate / 100)
                .flat_map(|n| {
                    let v = (n as f64 * hz * std::f64::consts::TAU / rate as f64).sin();
                    let s = (v * amplitude) as f32;
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

/// The encoder to test and a device to run it on: the software MFT on WARP,
/// or, for `hardware`, what `select::rank` picks on this machine's GPUs.
fn encoder_and_device(
    hardware: bool,
) -> Result<Result<(select::Encoder, device::Device), String>, String> {
    let encoders = match encode::h264_encoders() {
        Ok(encoders) => encoders,
        Err(e) => return Ok(Err(format!("no encoder list: {e}"))),
    };
    let names: Vec<&str> = encoders.iter().map(|e| e.name.as_str()).collect();
    if !hardware {
        let Some(software) = encoders.iter().find(|e| !e.hardware) else {
            return Ok(Err(format!("no software H.264 MFT on this image (offered: {names:?})")));
        };
        return Ok(Ok((software.clone(), device::create_warp_device()?)));
    }
    let adapters = device::adapters()?;
    let infos: Vec<select::Adapter> = adapters.iter().map(|a| a.info.clone()).collect();
    match select::rank(&infos, &encoders) {
        Choice::Hardware { encoder, adapter } => {
            let device = device::create_device(&adapters[adapter].adapter)?;
            Ok(Ok((encoders[encoder].clone(), device)))
        }
        other => Ok(Err(format!("no hardware encoder is ranked here ({other:?}; offered {names:?})"))),
    }
}

/// The path a recording takes from the slots to the file, `output::Output`,
/// fed `TICKS` ticks of one synthetic frame and, for each track of `layout`,
/// synthetic sources through a [`mix::TrackMix`] into that track's AAC
/// encoder, interleaved tick by tick as the session does it, each packet
/// handed over once the video has passed its end. Source 0 is a 440 Hz tone
/// throughout; source 1 is 660 Hz from 0.5 s to 1.5 s only, so the mix has a
/// source joining late and one going quiet; source 2 is 880 Hz throughout.
/// `hardware` paces the ticks in real time, as an asynchronous encoder is
/// fed. Media Foundation must already be started.
fn write_synthetic(out: &Path, layout: &AudioLayout, hardware: bool) -> Result<Outcome, String> {
    let (encoder, device) = match encoder_and_device(hardware)? {
        Ok(found) => found,
        Err(why) => return Ok(Outcome::Skipped(why)),
    };
    let slots = capture::create_slots(&device.device, W, H, 2)?;
    paint(&device, &slots[0]);

    let size = Size::new(W, H);
    let tracks = layout.tracks.len();
    let mut output =
        output::Output::create(out, &device, size, session::FPS, &encoder, slots.len(), tracks)?;
    let loaded = output.loaded().clone();
    let how = output.describe();
    let origin = 1_000 * clock::HNS_PER_SECOND;
    let packets_total = (TICKS / u64::from(session::FPS)) as i64 * 100;
    let mut queues: Vec<std::collections::VecDeque<feed::Packet>> = (0..layout.sources.len())
        .map(|i| match i {
            0 => tone_packets(origin, 440.0, 0.25, 0, packets_total),
            1 => tone_packets(origin, 660.0, 0.25, 50, 150),
            _ => tone_packets(origin, 880.0, 0.25, 0, packets_total),
        })
        .map(Into::into)
        .collect();
    let mut mix = mix::TrackMix::new(audio::SAMPLE_RATE, origin, layout);
    let started = Instant::now();
    for k in 0..TICKS {
        let t = clock::tick_time(k, session::FPS);
        let d = clock::tick_time(k + 1, session::FPS) - t;
        if hardware {
            let due = Duration::from_nanos(t as u64 * 100);
            while started.elapsed() < due {
                output.poll().map_err(|e| format!("{e} (polling before tick {k})"))?;
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        output.write(&device, &slots, 0, t, d).map_err(|e| format!("{e} (at tick {k})"))?;
        let video_end = t + d;
        for (i, queue) in queues.iter_mut().enumerate() {
            while let Some(packet) = queue.pop_front() {
                let end = clock::samples_at(packet.hns - origin, audio::SAMPLE_RATE)
                    + i64::from(packet.frames);
                if end > clock::samples_at(video_end, audio::SAMPLE_RATE) {
                    queue.push_front(packet);
                    break;
                }
                mix.push(i, packet);
            }
        }
        mix.write(video_end, video_end, &mut |track: usize, pcm: &[i16], position: u64| {
            output.write_audio(track, pcm, position)
        })?;
    }
    let end = clock::tick_time(TICKS, session::FPS);
    let finished = mix.finish(end, &mut |track: usize, pcm: &[i16], position: u64| {
        output.write_audio(track, pcm, position)
    });
    let mut lines = Vec::new();
    let expected = u64::from(audio::SAMPLE_RATE) * TICKS / u64::from(session::FPS);
    for (t, result) in finished.into_iter().enumerate() {
        result.map_err(|e| format!("audio track {t}: {e}"))?;
        let mixer = mix.track(t).mixer();
        if mixer.emitted() != expected {
            return Err(format!("audio track {t} wrote {} frames, not {expected}", mixer.emitted()));
        }
        lines.push(format!(
            "a:{t} {} ({} blocks, {} clipped)",
            layout.tracks[t].label, mixer.stats.blocks, mixer.stats.clipped
        ));
    }
    let stats = output.finalize()?;
    if stats.video_frames != TICKS {
        return Err(format!("{} video frames reached the file, not {TICKS}", stats.video_frames));
    }
    drop(slots);
    Ok(Outcome::Written {
        encoder: format!(
            "{} (hardware: {})",
            loaded.name.as_deref().unwrap_or("(no name)"),
            loaded.hardware()
        ),
        how,
        tracks: format!(
            "{} keyframes, {} fragments, AAC frames {:?}; {}",
            stats.keyframes,
            stats.fragments,
            stats.audio_frames,
            if lines.is_empty() { "no audio".to_string() } else { lines.join(", ") }
        ),
    })
}

/// No audio at all (every source failed to open): a video-only file, one
/// track, fragmented, with an `mfra`.
#[test]
fn the_software_encoder_writes_a_video_only_file() {
    let layout = AudioLayout { sources: vec![], tracks: vec![] };
    encode_test("video-only", &layout, false);
}

/// The Game preset: one AAC track, fed one source through the mixer.
#[test]
fn the_software_encoder_writes_the_game_track() {
    encode_test("game", &AudioPreset::Game.layout(), false);
}

/// Game + mic: the mix and two stems, a four-track file (video and three
/// audio), which Media Foundation's sink writer could not hold (#239).
#[test]
fn the_software_encoder_writes_a_four_track_file() {
    encode_test("game-mic", &AudioPreset::GameMic { mic_device_id: None }.layout(), false);
}

/// Game + mic + Discord: the mix and three stems, five tracks.
#[test]
fn the_software_encoder_writes_every_stem_of_game_mic_discord() {
    let layout = AudioPreset::GameMicDiscord { mic_device_id: None }.layout();
    encode_test("game-mic-discord", &layout, false);
}

/// The asynchronous path: the hardware encoder `select::rank` picks on this
/// machine, driven by its events, with NV12 textures from the video
/// processor, and the Game + mic + Discord layout. A runner has no GPU
/// encoder, so this runs on the box, by hand.
#[test]
#[ignore = "needs a hardware H.264 encoder; run by hand on a Windows box with a GPU"]
fn hardware_encoder_writes_every_track() {
    let layout = AudioPreset::GameMicDiscord { mic_device_id: None }.layout();
    encode_test("hardware", &layout, true);
}

fn encode_test(name: &str, layout: &AudioLayout, hardware: bool) {
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
                let outcome = write_synthetic(&out, layout, hardware);
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
            if hardware {
                panic!("the hardware test could not run: {why}");
            }
            report(&format!("SKIPPED the encoding test ({name}): {why}"));
        }
        Ok(Outcome::Written { encoder, how, tracks }) => {
            let mut file = File::open(&out).expect("no file was written");
            let summary = mp4::summarize(&mut file).expect("summarize");
            report(&format!(
                "RAN the encoding test ({name}): {TICKS} ticks via {encoder}, {how}; {tracks}; \
                 {} bytes, layout {}",
                summary.file_len, summary.layout
            ));
            let audio = layout.tracks.len() as u32;
            assert!(summary.structurally_playable(), "{summary:?}");
            assert!(summary.mvex, "not fragmented: {summary:?}");
            assert!(summary.mfra, "no mfra: {summary:?}");
            assert_eq!(summary.truncated, None, "{summary:?}");
            assert_eq!(summary.trailing_garbage, 0, "{summary:?}");
            assert_eq!(summary.tracks, 1 + audio, "video and {audio} audio: {summary:?}");
            assert_eq!(summary.audio_tracks, audio, "{summary:?}");
            // One fragment per GOP: at least the three keyframes of five
            // seconds at GOP 120, if the encoder honoured the GOP.
            assert!(summary.complete_fragments >= 1, "{summary:?}");
            // A finished file is left alone by the repair recovery runs.
            assert!(mp4::write::repair(&out).expect("repair").already_complete);
            decode(&out, 1 + audio);
        }
        Err(e) => panic!("the encoding path failed: {e}"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// A full decode of every stream with ffmpeg, if one is on `PATH`, which the
/// hosted runner's image may or may not have. Reports which way it went.
fn decode(file: &Path, streams: u32) {
    let ffmpeg = Path::new("ffmpeg");
    let probe = crate::ffmpeg_command(ffmpeg).arg("-version").output();
    if !probe.is_ok_and(|o| o.status.success()) {
        report("no ffmpeg on PATH, so no decode");
        return;
    }
    let out = crate::ffmpeg_command(ffmpeg)
        .args(["-v", "error", "-i"])
        .arg(file)
        .args(["-map", "0", "-f", "null", "-"])
        .output()
        .expect("ffmpeg ran");
    let errors = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success() && errors.trim().is_empty(), "decode failed: {errors}");
    report(&format!("decoded all {streams} stream(s) of {} with ffmpeg, cleanly", file.display()));
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

/// One frame of `content` size through `fitter` into a 1920x1080 slot that
/// starts white, so a bar that was not cleared shows as white rather than as
/// a texture's initial zeroes. Returns what the fitter did and the slot's
/// pixels.
fn fit_one(
    device: &device::Device,
    content: Size,
    mut fitter: scale::Fitter,
) -> Result<(scale::Placed, Vec<u8>), String> {
    let source = scale::create_bgra(device, content)?;
    fill(device, &source, content, CONTENT);
    let slots = capture::create_slots(&device.device, OUTPUT.width, OUTPUT.height, 1)?;
    fill(device, &slots[0].texture, OUTPUT, WHITE);
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
    let (placed, px) =
        fit_one(&device, Size::new(1280, 720), scale::Fitter::new(OUTPUT)).expect("1280x720");
    assert_eq!(placed, scale::Placed::Scaled);
    for (x, y) in [(2, 2), (1917, 2), (2, 1077), (1917, 1077), (960, 540)] {
        assert_near(pixel(&px, OUTPUT, x, y), CONTENT, &format!("1280x720 at ({x}, {y})"));
    }

    // 5:4: 1350x1080 at x = 284, black either side.
    let (placed, px) =
        fit_one(&device, Size::new(1280, 1024), scale::Fitter::new(OUTPUT)).expect("1280x1024");
    assert_eq!(placed, scale::Placed::Scaled);
    for (x, y) in [(0, 0), (100, 540), (280, 1079), (1640, 0), (1820, 540), (1919, 1079)] {
        assert_near(pixel(&px, OUTPUT, x, y), BLACK, &format!("bar at ({x}, {y})"));
    }
    for (x, y) in [(290, 2), (960, 540), (1628, 1077)] {
        assert_near(pixel(&px, OUTPUT, x, y), CONTENT, &format!("1280x1024 at ({x}, {y})"));
    }

    report(&format!("RAN the video-processor test on {name}: scaled and letterboxed"));
}

/// What needs no video processor, on WARP, which the runner always has: the
/// recording's own size is a plain copy, a resized frame falls back to a crop
/// over black (never stale pixels) when the processor is unavailable, and
/// `fill_black` is what the session writes after the window closes.
#[test]
fn a_frame_is_copied_or_cropped_over_black_without_a_video_processor() {
    let device = device::create_warp_device().expect("a WARP device");

    let (placed, px) = fit_one(&device, OUTPUT, scale::Fitter::new(OUTPUT)).expect("1920x1080");
    assert_eq!(placed, scale::Placed::Copied);
    assert_near(pixel(&px, OUTPUT, 0, 0), CONTENT, "copied corner");
    assert_near(pixel(&px, OUTPUT, 1919, 1079), CONTENT, "copied corner");

    let cropping = scale::Fitter::cropping(OUTPUT, "no processor, for the test");
    let (placed, px) = fit_one(&device, Size::new(1280, 1024), cropping).expect("1280x1024");
    assert_eq!(placed, scale::Placed::Cropped);
    for (x, y) in [(0, 0), (100, 540), (1279, 1023)] {
        assert_near(pixel(&px, OUTPUT, x, y), CONTENT, &format!("cropped at ({x}, {y})"));
    }
    // The slot started white: what the frame does not reach is black.
    for (x, y) in [(1280, 0), (1500, 540), (100, 1024), (1919, 1079)] {
        assert_near(pixel(&px, OUTPUT, x, y), BLACK, &format!("outside the crop at ({x}, {y})"));
    }

    let slots = capture::create_slots(&device.device, OUTPUT.width, OUTPUT.height, 1)
        .expect("slot");
    fill(&device, &slots[0].texture, OUTPUT, WHITE);
    scale::fill_black(&device, &slots[0].texture).expect("fill_black");
    let px = read_back(&device, &slots[0].texture, OUTPUT);
    assert_near(pixel(&px, OUTPUT, 960, 540), BLACK, "after fill_black");

    report("RAN the WARP copy, crop-over-black and fill_black test");
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
/// two seconds, stop, through a real capture worker. Needs a desktop session
/// for WGC, so it is ignored until it proves stable on a runner.
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
    // The capture runs in the worker, which is `ninja-recorder.exe` itself:
    // this test's own executable is the harness, so name the built binary
    // beside it (`target/<profile>/deps/..`). `cargo build` first.
    let exe = std::env::current_exe()
        .ok()
        .and_then(|harness| Some(harness.parent()?.parent()?.join("ninja-recorder.exe")))
        .filter(|exe| exe.exists())
        .expect("no ninja-recorder.exe beside the test harness: run `cargo build` first");
    let mut recorder = super::OwnRecorder::with_worker(Some(exe), None);
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
    // the Game preset's one source cannot open: video only, and reported as
    // such.
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
    let target = audio::Target::Process(std::process::id());
    let outcome = audio::start("game", target, tx).map(|source| {
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

/// The default microphone and the desktop in loopback, with its keep-alive,
/// open at 48 kHz stereo float through the engine's own conversion and
/// deliver for two seconds (#238). Needs an audio engine and devices, so it
/// is ignored on the runner. A machine with no microphone reports that and
/// still passes on the desktop alone.
#[test]
#[ignore = "needs an audio engine, a microphone and an output device; run by hand on a Windows box"]
fn endpoints_capture() {
    let mut opened = 0;
    for (name, target) in [
        ("microphone", audio::Target::Microphone(None)),
        ("desktop", audio::Target::Desktop),
    ] {
        let (tx, rx) = std::sync::mpsc::channel();
        let outcome = audio::start(name, target, tx).map(|source| {
            std::thread::sleep(std::time::Duration::from_secs(2));
            source.stop()
        });
        let received: Vec<feed::Packet> = rx.try_iter().collect();
        let frames: u64 = received.iter().map(|p| u64::from(p.frames)).sum();
        match outcome {
            Ok(Ok(summary)) => {
                opened += 1;
                report(&format!(
                    "{name} ran: {} packets ({frames} frames, {:.2} s), clock {:?}, {} silent",
                    summary.packets,
                    frames as f64 / f64::from(audio::SAMPLE_RATE),
                    summary.clock,
                    summary.silent_packets
                ));
                assert!(summary.packets > 0, "{name} delivered nothing in two seconds");
            }
            Ok(Err(e)) => panic!("{name} started, then failed: {e}"),
            Err(e) => report(&format!("{name} did not open: {e}")),
        }
    }
    assert!(opened > 0, "neither endpoint opened");
}
