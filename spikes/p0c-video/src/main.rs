//! **P0c stage 2 (WS1.4, #8): WGC frames into a fragmented MP4, and what a
//! kill leaves behind.**
//!
//! The plan's exit criterion (§4.5) is three clauses, and this binary exists
//! to turn each into a printed number rather than an impression:
//!
//! 1. **Drift under one frame over ten minutes.** Video is written on a 60 fps
//!    grid kept by the performance counter, repeating the last WGC frame when
//!    nothing new arrived; audio comes from a WASAPI endpoint whose own clock
//!    counts the samples. The run measures how far the device clock walks
//!    away from the video's, corrects it by slipping single samples, and then
//!    measures the file to see what the encoder and muxer did to the result.
//!    `clock.rs` holds the arithmetic and is tested on any host.
//! 2. **A file killed at minute five is playable.** `--kill-after 300` runs
//!    the capture as a child and terminates it with `TerminateProcess`, which
//!    is what Task Manager's End task does: no destructor, no finalize, no
//!    flush. Then it checks what is on disk (`verify.rs`): the MP4's boxes
//!    read directly, a full decode with ffmpeg, and the app's own faststart
//!    remux.
//! 3. **Encoder detection.** `--list` enumerates the GPUs with their PCI vendor
//!    IDs and every H.264 encoder Media Foundation offers, hardware and
//!    software; the run reports the one the sink writer actually loaded and
//!    whether its vendor matches the adapter. `--encoder hardware` refuses to
//!    run on a machine with none, which is the plan's "refuse on none".
//!
//! **No injection, anywhere.** Frames come from Windows.Graphics.Capture, the
//! same public API the Snipping Tool uses; nothing is loaded into the game.
//! That is a hard constraint, not a preference (DEVELOPMENT.md §1.1).
//!
//! `README.md` next to this file is the run guide for #8. It has never been
//! built on Windows or run; everything here is checked from Linux with
//! `cargo check --target x86_64-pc-windows-msvc`, which does not link.
//!
//! ## What it is deliberately not
//!
//! Not `recorder/own/`. The BGRA-to-NV12 conversion is left to the video
//! processor the sink writer inserts rather than a shader of our own; the
//! audio is one endpoint rather than the mic, loopback and system mix on one
//! clock; the drift correction slips samples rather than resampling. Each is a
//! thing WS1.6 has to build properly, and each is chosen here because it
//! cannot make a pass look better than the real pipeline would.

// The capture half is Windows-only, so off Windows these are exercised by
// their tests and nothing else.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
mod clock;
mod mp4;
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
mod pcm;
mod probe;
mod verify;

#[cfg(target_os = "windows")]
mod win;

use std::path::PathBuf;

pub const FPS: u32 = 60;

#[derive(Clone, Debug)]
pub enum Target {
    /// The game window, found by class (`RiotWindowClass`) the way
    /// `recorder/libobs/window.rs` finds it.
    Game,
    Window(String),
    Monitor(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoder {
    /// Hardware H.264 only. Refuses to start without one, and fails the run
    /// if the sink writer quietly loaded a software encoder instead.
    Hardware,
    /// Hardware transforms disabled: Microsoft's software H.264 encoder.
    /// #68's second arm, and what every machine without a supported GPU gets.
    Software,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioSource {
    /// The default render endpoint, in loopback: what the speakers play.
    System,
    /// The default capture endpoint: usually a USB headset, whose crystal is
    /// the one most likely to disagree with the machine's.
    Mic,
    None,
}

#[derive(Clone, Debug)]
pub struct Args {
    pub list: bool,
    pub verify: Option<PathBuf>,
    pub target: Target,
    pub seconds: u64,
    pub out: PathBuf,
    pub encoder: Encoder,
    pub adapter: u32,
    pub audio: AudioSource,
    pub audio_clock: clock::AudioClock,
    pub kill_after: Option<u64>,
    pub ffmpeg: Option<PathBuf>,
    pub no_verify: bool,
    /// Internal: this process is the capture half of a `--kill-after` run,
    /// and terminates itself at that point instead of spawning anything.
    pub child: bool,
}

pub fn usage() -> String {
    "p0c-video - WS1.4 (#8), the WGC + Media Foundation spike\n\
     \n\
     What to capture (default: the League game window, by class RiotWindowClass):\n\
       --window <text>      a window whose title contains this\n\
       --monitor <n>        a display, by its index in --list\n\
     \n\
     The run:\n\
       --seconds <n>        how long (default 600, the exit criterion's ten minutes)\n\
       --out <path>         the MP4 (default p0c-video.mp4; overwritten)\n\
       --encoder <e>        hardware (default; refuses without one) or software\n\
       --adapter <n>        the GPU to capture and encode on, from --list (default 0)\n\
       --audio <a>          system (default: loopback of the default output), mic, none\n\
       --audio-clock <c>    qpc (default: corrected onto the video clock) or device\n\
                            (uncorrected sample count, to see the raw drift in the file)\n\
       --kill-after <s>     run the capture as a child and TerminateProcess it at <s>\n\
                            seconds, as Task Manager's End task would, then check the file\n\
     \n\
     Checking:\n\
       --verify <file>      check an existing file and exit (works on any OS)\n\
       --ffmpeg <path>      the ffmpeg to decode with (default: the installed app's\n\
                            bundled copy, then PATH)\n\
       --no-verify          skip the file check after a run\n\
     \n\
       --list               GPUs, H.264 encoders, monitors and windows; captures nothing\n\
     \n\
     spikes/p0c-video/README.md is the procedure for #8."
        .to_string()
}

fn value<I: Iterator<Item = String>>(args: &mut I, flag: &str) -> Result<String, String> {
    args.next()
        .ok_or_else(|| format!("{flag} needs a value\n\n{}", usage()))
}

fn number<T: std::str::FromStr>(text: &str, flag: &str) -> Result<T, String> {
    text.parse()
        .map_err(|_| format!("{flag} wants a number, got {text:?}"))
}

pub fn parse_args<I: Iterator<Item = String>>(mut args: I) -> Result<Args, String> {
    let mut parsed = Args {
        list: false,
        verify: None,
        target: Target::Game,
        seconds: 600,
        out: PathBuf::from("p0c-video.mp4"),
        encoder: Encoder::Hardware,
        adapter: 0,
        audio: AudioSource::System,
        audio_clock: clock::AudioClock::Qpc,
        kill_after: None,
        ffmpeg: None,
        no_verify: false,
        child: false,
    };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--list" => parsed.list = true,
            "--verify" => parsed.verify = Some(PathBuf::from(value(&mut args, &arg)?)),
            "--window" => parsed.target = Target::Window(value(&mut args, &arg)?),
            "--monitor" => parsed.target = Target::Monitor(number(&value(&mut args, &arg)?, &arg)?),
            "--seconds" => parsed.seconds = number(&value(&mut args, &arg)?, &arg)?,
            "--out" => parsed.out = PathBuf::from(value(&mut args, &arg)?),
            "--encoder" => {
                parsed.encoder = match value(&mut args, &arg)?.as_str() {
                    "hardware" => Encoder::Hardware,
                    "software" => Encoder::Software,
                    other => {
                        return Err(format!("--encoder is hardware or software, not {other:?}"));
                    }
                }
            }
            "--adapter" => parsed.adapter = number(&value(&mut args, &arg)?, &arg)?,
            "--audio" => {
                parsed.audio = match value(&mut args, &arg)?.as_str() {
                    "system" => AudioSource::System,
                    "mic" => AudioSource::Mic,
                    "none" => AudioSource::None,
                    other => return Err(format!("--audio is system, mic or none, not {other:?}")),
                }
            }
            "--audio-clock" => {
                parsed.audio_clock = match value(&mut args, &arg)?.as_str() {
                    "qpc" => clock::AudioClock::Qpc,
                    "device" => clock::AudioClock::Device,
                    other => return Err(format!("--audio-clock is qpc or device, not {other:?}")),
                }
            }
            "--kill-after" => parsed.kill_after = Some(number(&value(&mut args, &arg)?, &arg)?),
            "--ffmpeg" => parsed.ffmpeg = Some(PathBuf::from(value(&mut args, &arg)?)),
            "--no-verify" => parsed.no_verify = true,
            "--child" => parsed.child = true,
            "--help" | "-h" => return Err(usage()),
            other => return Err(format!("unknown argument {other:?}\n\n{}", usage())),
        }
    }
    if parsed.seconds == 0 {
        return Err("--seconds must be at least 1".to_string());
    }
    if let Some(kill) = parsed.kill_after
        && kill >= parsed.seconds
    {
        return Err(format!(
            "--kill-after {kill} is not before --seconds {}: the run would finish cleanly first",
            parsed.seconds
        ));
    }
    Ok(parsed)
}

/// `--verify <file>`: the file check on its own, which is also what to run
/// after killing a capture from Task Manager by hand.
fn verify_only(args: &Args, file: &std::path::Path) -> Result<(), String> {
    let findings = verify::check(
        file,
        args.ffmpeg.as_deref(),
        args.audio != AudioSource::None,
    )?;
    print!("{}", verify::render(file, &findings, FPS));
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn main() -> std::process::ExitCode {
    let result = parse_args(std::env::args().skip(1)).and_then(|args| match &args.verify {
        Some(file) => verify_only(&args, file),
        None => Err(
            "p0c-video captures with Windows.Graphics.Capture, which exists only on \
                     Windows.\nIt is checked elsewhere (cargo check --target \
                     x86_64-pc-windows-msvc) and run on the box.\n--verify <file> works here."
                .to_string(),
        ),
    });
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("p0c-video: {err}");
            std::process::ExitCode::from(2)
        }
    }
}

#[cfg(target_os = "windows")]
fn main() -> std::process::ExitCode {
    let result = parse_args(std::env::args().skip(1)).and_then(|args| match &args.verify {
        Some(file) => verify_only(&args, file),
        None => win::run(&args),
    });
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("p0c-video: {err}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(line: &str) -> Result<Args, String> {
        parse_args(line.split_whitespace().map(str::to_string))
    }

    #[test]
    fn the_defaults_are_the_gate_run() {
        let a = parse("").unwrap();
        assert!(matches!(a.target, Target::Game));
        assert_eq!(a.seconds, 600);
        assert_eq!(a.encoder, Encoder::Hardware);
        assert_eq!(a.audio, AudioSource::System);
        assert_eq!(a.audio_clock, clock::AudioClock::Qpc);
        assert_eq!(a.kill_after, None);
    }

    #[test]
    fn a_kill_must_come_before_the_end() {
        assert!(parse("--kill-after 300").is_ok());
        assert!(parse("--seconds 300 --kill-after 300").is_err());
    }

    #[test]
    fn bad_values_are_refused() {
        assert!(parse("--encoder nvenc").is_err());
        assert!(parse("--monitor one").is_err());
        assert!(parse("--seconds 0").is_err());
        assert!(parse("--wat").is_err());
    }
}
