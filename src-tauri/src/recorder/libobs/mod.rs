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

use super::audio::{AudioLayout, AudioSourceKind};
use super::{RecordConfig, Recorder, RecorderError, RecordingOutput};
use libobs_recorder::settings::{
    AudioTrackSource as ObsAudioSource, AudioTrack as ObsAudioTrack, Encoder, Framerate, RateControl,
    RecorderSettings, Resolution, Window,
};
use libobs_recorder::Recorder as LibObs;
use std::path::{Path, PathBuf};
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
    /// `None` while the backend is cold. Bringing `LibObs` up spawns the
    /// out-of-process worker *and* sends it `Init`, which runs
    /// `obs_startup` and loads every plugin — so a live `Some` here is a
    /// D3D11 device and the whole libobs plugin set resident in another
    /// process, not a dormant handle. Holding that from launch to exit put
    /// the largest single item on the idle-RAM budget (DEVELOPMENT.md
    /// §1.2) for a machine that might never open League, so it is now tied
    /// to the state machine's `ClientRunning` window instead. See
    /// `prepare`/`release`.
    inner: Option<LibObs>,
    /// Kept so `ensure_up` can rebuild `inner` after a `release`.
    extprocess_recorder_path: PathBuf,
    /// Why the last bring-up failed, for `backend_name`. Init failure used
    /// to happen once at startup and swap in a `FailedRecorder`; now it can
    /// happen at any `prepare`, so the diagnostic has to live here.
    last_error: Option<String>,
    active_path: Option<PathBuf>,
    /// The layout `start` actually configured, held so `stop` can report it
    /// without re-deriving it from a preference that may have changed
    /// mid-game.
    active_audio: Option<AudioLayout>,
    /// Resolved by the caller the same way as `extprocess_recorder_path`
    /// (Tauri's path resolver, bundled as a resource) — `None` if it
    /// isn't present, in which case `stop` just skips the faststart
    /// remux and leaves the crash-safe fragmented file as-is. Recording
    /// must never fail just because this optional finishing step is
    /// unavailable.
    ffmpeg_path: Option<PathBuf>,
}

impl LibObsRecorder {
    /// `extprocess_recorder_path` is the path to the out-of-process libobs
    /// worker binary that `libobs-recorder` spawns and talks to over IPC —
    /// crash isolation, so a libobs crash doesn't take the whole app down
    /// mid-game. It has to be resolved by the caller via Tauri's path
    /// resolver (`BaseDirectory::Resource`) since dev and installed-app
    /// layouts differ; see the `build.rs`/`tauri.conf.json` comments for
    /// how it gets bundled next to the binary.
    /// Cheap and infallible — it only records paths. The backend itself
    /// comes up on the first `prepare` or `start`, so construction no
    /// longer decides whether recording works; `backend_name` reports that.
    pub fn new(extprocess_recorder_path: PathBuf, ffmpeg_path: Option<PathBuf>) -> Self {
        Self {
            inner: None,
            extprocess_recorder_path,
            last_error: None,
            active_path: None,
            active_audio: None,
            ffmpeg_path,
        }
    }

    /// Brings the backend up if it is cold, and hands back a live handle.
    /// Idempotent, so `start` can call it unconditionally without caring
    /// whether `prepare` already ran or succeeded.
    fn ensure_up(&mut self) -> Result<&mut LibObs, RecorderError> {
        if self.inner.is_none() {
            // Cloned rather than borrowed so the call can't hold a shared
            // borrow of `self` across the assignments below. Once per cold
            // bring-up, next to spawning a process — not worth being clever.
            let worker = self.extprocess_recorder_path.clone();
            match LibObs::new_with_paths(Some(worker), None, None, None) {
                Ok(obs) => {
                    self.last_error = None;
                    self.inner = Some(obs);
                }
                Err(e) => {
                    let message = e.to_string();
                    self.last_error = Some(message.clone());
                    return Err(RecorderError::Backend(message));
                }
            }
        }
        Ok(self
            .inner
            .as_mut()
            .expect("inner was just initialized or already Some"))
    }

    /// Shuts the worker down and goes cold. Dropping `LibObs` would also
    /// terminate the child (its IPC link kills it on drop), but `shutdown`
    /// asks politely first so libobs gets to release the GPU device.
    fn tear_down(&mut self) {
        let Some(obs) = self.inner.take() else {
            return;
        };
        if let Err(e) = obs.shutdown() {
            // Nothing to do about it — the link's `Drop` kills the child
            // regardless, and we are on our way to idle either way.
            eprintln!("[recorder] libobs shutdown failed, dropping anyway: {e}");
        }
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

        // One mp4 audio track per entry, in order: track 0 is the combined
        // mix, the rest are isolated stems (DEVELOPMENT.md §2.5). The fork
        // turns each track's source list into a libobs mixer bitmask —
        // a source can feed several mixes at once, which is what makes the
        // stems nearly free.
        let audio = config.audio.layout();
        if let Err(e) = audio.validate() {
            return Err(RecorderError::Backend(format!("invalid audio layout: {e}")));
        }
        settings.set_audio_tracks(to_obs_tracks(&audio));

        let obs = self.ensure_up()?;
        let encoder = obs
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

        obs.configure(&settings)
            .map_err(|e| RecorderError::Backend(e.to_string()))?;
        obs.start_recording()
            .map_err(|e| RecorderError::Backend(e.to_string()))?;

        self.active_path = Some(output_path);
        self.active_audio = Some(audio);
        Ok(())
    }

    fn stop(&mut self) -> Result<RecordingOutput, RecorderError> {
        let path = self.active_path.take().ok_or(RecorderError::NotRecording)?;
        let audio = self.active_audio.take().unwrap_or_else(|| {
            // Can't happen — the two are set together in `start` — but
            // guessing a layout is worse than reporting an empty one, which
            // the library records as "unknown".
            AudioLayout { sources: Vec::new(), tracks: Vec::new() }
        });
        self.inner
            .as_mut()
            // `active_path` was Some, so `start` succeeded and brought the
            // backend up. Anything else is a bug, not a user-facing state.
            .ok_or_else(|| {
                RecorderError::Backend("capture backend went away mid-recording".into())
            })?
            .stop_recording()
            .map_err(|e| RecorderError::Backend(e.to_string()))?;

        // The fork's muxer settings (see its `intprocess-recorder` source)
        // write `movflags=frag_keyframe+empty_moov+default_base_moof` —
        // deliberately crash-safe (no finalize step needed, so a libobs
        // crash mid-game doesn't corrupt the file) but with no upfront
        // seek index, which makes most players — including the review
        // UI's own WebView2 `<video>` — unable to scrub it reliably. Only
        // worth fixing up now, on a clean stop; a stream-copy remux is
        // fast and lossless, just rewriting the container's index.
        if let Some(ffmpeg_path) = &self.ffmpeg_path {
            if let Err(e) = remux_faststart(ffmpeg_path, &path, audio.tracks.len()) {
                eprintln!(
                    "[recorder] faststart remux failed, keeping original (unseekable) file: {e}"
                );
            }
        }

        Ok(RecordingOutput { path, audio })
    }

    fn is_recording(&self) -> bool {
        self.active_path.is_some()
    }

    fn backend_name(&self) -> String {
        match (&self.inner, &self.last_error) {
            (Some(_), _) => "libobs (ready)".to_string(),
            (None, Some(e)) => format!("libobs (unavailable: {e})"),
            (None, None) => "libobs (idle)".to_string(),
        }
    }

    fn prepare(&mut self) -> Result<(), RecorderError> {
        self.ensure_up().map(|_| ())
    }

    fn release(&mut self) {
        if self.is_recording() {
            // The state machine shouldn't ask for this mid-recording, but
            // tearing libobs down under a live encoder would lose the game.
            return;
        }
        self.tear_down();
    }
}

/// Translates our backend-agnostic layout into the fork's own track type.
///
/// The only place libobs vocabulary meets ours, which is the point of the
/// `Recorder` trait boundary (DEVELOPMENT.md §2.2): `AudioLayout` crosses
/// the Tauri IPC and lands in SQLite, so it must not grow a libobs type.
///
/// Source order is preserved because `AudioTrackSpec::sources` indexes into
/// it — reordering here would silently reassign every stem.
fn to_obs_tracks(layout: &AudioLayout) -> Vec<ObsAudioTrack> {
    let sources: Vec<ObsAudioSource> = layout
        .sources
        .iter()
        .map(|source| match source {
            AudioSourceKind::Game => ObsAudioSource::Application { exe: None },
            AudioSourceKind::Application { exe } => ObsAudioSource::Application {
                exe: Some(exe.clone()),
            },
            AudioSourceKind::Desktop => ObsAudioSource::Output { device_id: None },
            AudioSourceKind::Microphone { device_id } => ObsAudioSource::Input {
                device_id: device_id.clone(),
            },
        })
        .collect();

    layout
        .tracks
        .iter()
        .map(|track| ObsAudioTrack {
            name: track.label.clone(),
            sources: track.sources.iter().filter_map(|&i| sources.get(i).cloned()).collect(),
        })
        .collect()
}

/// Stream-copies `video_path` through ffmpeg with `-movflags +faststart`
/// so the `moov` (seek index) ends up at the front of the file instead of
/// wherever the fragmented writer left it, then atomically replaces the
/// original. `-c copy` means no re-encode — this only rewrites container
/// metadata, so it's a fraction of a second even for a long recording.
fn remux_faststart(
    ffmpeg_path: &Path,
    video_path: &Path,
    audio_track_count: usize,
) -> Result<(), String> {
    let tmp_path = video_path.with_extension("faststart.tmp.mp4");

    let mut command = crate::ffmpeg_command(ffmpeg_path);
    command
        .arg("-y") // overwrite tmp_path without prompting if it's left over from a previous crash
        .arg("-i")
        .arg(video_path)
        // Without these, ffmpeg's *default* stream selection applies: one
        // video and one audio stream, the "best" of each. On a multi-track
        // recording that silently drops every stem, and the rename below
        // makes it permanent. `0:a?` rather than `0` also skips any
        // data/unknown stream without needing `-ignore_unknown`, and the
        // `?` keeps an audio-less file from failing outright.
        .args(["-map", "0:v?", "-map", "0:a?"])
        .args(["-c", "copy", "-movflags", "+faststart"]);

    // obs-ffmpeg-mux never sets AV_DISPOSITION_DEFAULT, so without this it
    // is ambiguous which track a player picks. Track 0 is the combined mix
    // and must be the one that plays by default.
    for track in 0..audio_track_count.max(1) {
        command
            .arg(format!("-disposition:a:{track}"))
            .arg(if track == 0 { "default" } else { "0" });
    }

    let output = command
        .arg(&tmp_path)
        .output()
        .map_err(|e| format!("failed to launch ffmpeg at {}: {e}", ffmpeg_path.display()))?;

    if !output.status.success() {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(format!(
            "ffmpeg exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    std::fs::rename(&tmp_path, video_path)
        .map_err(|e| format!("failed to replace original file with remuxed one: {e}"))
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
