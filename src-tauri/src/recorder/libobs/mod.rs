//! Windows capture backend: libobs via the (forked, patched) `libobs-recorder`
//! crate. See DEVELOPMENT.md §2.1/§2.2 for why libobs is embedded at all,
//! and the fork's readme (github.com/NinjaGoldfinch/libobs-recorder) for
//! why it can't be the unpatched upstream — upstream's video source is
//! `game_capture`, which hooks (DLL-injects) the target process. That's
//! exactly what Riot Vanguard exists to detect (§1.1). The fork forces
//! `window_capture` with the Windows.Graphics.Capture method instead: no
//! injection, DWM composited-frame capture only.
//!
//! **Unverified.** Written and reviewed against the reference
//! implementation's actual working code, but this crate only builds and
//! runs on Windows, and no Windows machine touched this file — same
//! caveat as the async supervisor glue in `state_machine::supervisor`.
//! Needs a real pass on the Windows box (DEVELOPMENT.md §9) before it can
//! be trusted: does `window_capture`+WGC actually produce frames for a
//! borderless/windowed League client, does the encoder priority pick what
//! we expect on real NVENC/AMF/QSV hardware, does Vanguard tolerate it.

mod window;

use super::{RecordConfig, Recorder, RecorderError};
use libobs_recorder::settings::{
    AudioSource, Encoder, Framerate, RateControl, RecorderSettings, Resolution, Window,
};
use libobs_recorder::Recorder as LibObs;
use std::path::PathBuf;
use std::time::Duration;
use window::{WINDOW_CLASS, WINDOW_PROCESS, WINDOW_TITLE};

/// Hardware H.264 encoders, in priority order (matches the crate's own
/// `Encoder` derive order — NVENC, then AMD AMF, then Intel QSV). AV1
/// variants are deliberately excluded even though the crate would offer
/// them ahead of some H.264 options: the review player (`src/review.ts`)
/// relies on WebView2's native `<video>` H.264 decode (DEVELOPMENT.md
/// §2.4), and `OBS_X264` is excluded so an all-software-only machine fails
/// loudly instead of silently recording with the CPU (§2.4: "no silent
/// x264 fallback on the gameplay machine").
fn is_acceptable_encoder(encoder: &Encoder) -> bool {
    matches!(
        encoder,
        Encoder::JIM_NVENC
            | Encoder::FFMPEG_NVENC
            | Encoder::AMD_AMF_H264
            | Encoder::OBS_QSV11_H264
    )
}

pub struct LibObsRecorder {
    inner: LibObs,
    active_path: Option<PathBuf>,
}

impl LibObsRecorder {
    /// `extprocess_recorder_path` is the path to the out-of-process libobs
    /// worker binary that `libobs-recorder` spawns and talks to over IPC —
    /// crash isolation, so a libobs crash doesn't take the whole app down
    /// mid-game. It has to be resolved by the caller via Tauri's path
    /// resolver (`BaseDirectory::Executable`) since dev and installed-app
    /// layouts differ; see the `build.rs`/`tauri.conf.json` comments for
    /// how it gets bundled next to the binary.
    pub fn new(extprocess_recorder_path: PathBuf) -> Result<Self, RecorderError> {
        let inner = LibObs::new_with_paths(Some(extprocess_recorder_path), None, None, None)
            .map_err(|e| RecorderError::Backend(e.to_string()))?;
        Ok(Self {
            inner,
            active_path: None,
        })
    }
}

impl Recorder for LibObsRecorder {
    fn start(&mut self, config: RecordConfig) -> Result<(), RecorderError> {
        if self.active_path.is_some() {
            return Err(RecorderError::AlreadyRecording);
        }

        std::fs::create_dir_all(&config.output_dir)?;
        let output_path = config.output_dir.join(format!("{}.mp4", config.file_stem));

        // By the time `Recorder::start` is called, the state machine has
        // already observed Live Client Data responding (DEVELOPMENT.md
        // §3.4), so the game window should exist — but it can take a beat
        // to report a real (non-1x1) client rect (see `window::window_size`).
        // Bounded retry rather than the reference implementation's 30s/60
        // attempts: `start` is called synchronously from state-machine
        // dispatch, not a background task, so a long block here would stall
        // it. Falls back to 1080p rather than failing the recording.
        let resolution = wait_for_window_size(10, Duration::from_millis(300))
            .unwrap_or_else(|| Resolution::new(1920, 1080));

        let mut settings = RecorderSettings::new(
            Window::new(
                WINDOW_TITLE,
                Some(WINDOW_CLASS.into()),
                Some(WINDOW_PROCESS.into()),
            ),
            resolution,
            resolution, // no scaling: capture at the window's native size
            &output_path,
        );
        settings.set_framerate(Framerate::new(60, 1));
        settings.set_rate_control(RateControl::CBR(8000)); // ~8 Mbps, DEVELOPMENT.md §2.4
        settings.set_audio_source(AudioSource::SYSTEM);

        let encoder = self
            .inner
            .available_encoders()
            .map_err(|e| RecorderError::Backend(e.to_string()))?
            .into_iter()
            .find(is_acceptable_encoder)
            .ok_or_else(|| {
                RecorderError::Backend(
                    "no hardware H.264 encoder available (NVENC/AMD AMF/Intel QSV) — refusing to fall \
                     back to software x264 on the gameplay machine, see DEVELOPMENT.md §2.4"
                        .into(),
                )
            })?;
        settings.set_encoder(encoder);

        self.inner
            .configure(&settings)
            .map_err(|e| RecorderError::Backend(e.to_string()))?;
        self.inner
            .start_recording()
            .map_err(|e| RecorderError::Backend(e.to_string()))?;

        self.active_path = Some(output_path);
        Ok(())
    }

    fn stop(&mut self) -> Result<PathBuf, RecorderError> {
        let path = self.active_path.take().ok_or(RecorderError::NotRecording)?;
        self.inner
            .stop_recording()
            .map_err(|e| RecorderError::Backend(e.to_string()))?;
        Ok(path)
    }

    fn is_recording(&self) -> bool {
        self.active_path.is_some()
    }
}

fn wait_for_window_size(max_attempts: u32, interval: Duration) -> Option<Resolution> {
    for attempt in 0..max_attempts {
        if let Some(size) = window::find_window().and_then(window::window_size) {
            return Some(size);
        }
        if attempt + 1 < max_attempts {
            std::thread::sleep(interval);
        }
    }
    None
}
