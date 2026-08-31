//! Dev/macOS `Recorder` backend. Does no real capture — it simulates
//! encoder start/stop latency and produces an output file, so the rest of
//! the app (state machine, VOD library, review UI) is fully developable
//! without libobs or Windows. See DEVELOPMENT.md §2.2 and §9.

use super::{RecordConfig, Recorder, RecorderError};
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

pub struct StubRecorder {
    active: Option<RecordConfig>,
}

impl StubRecorder {
    pub fn new() -> Self {
        Self { active: None }
    }
}

impl Default for StubRecorder {
    fn default() -> Self {
        Self::new()
    }
}

impl Recorder for StubRecorder {
    fn start(&mut self, config: RecordConfig) -> Result<(), RecorderError> {
        if self.active.is_some() {
            return Err(RecorderError::AlreadyRecording);
        }
        fs::create_dir_all(&config.output_dir)?;
        thread::sleep(Duration::from_millis(150)); // simulated encoder spin-up
        self.active = Some(config);
        Ok(())
    }

    fn stop(&mut self) -> Result<PathBuf, RecorderError> {
        let config = self.active.take().ok_or(RecorderError::NotRecording)?;
        thread::sleep(Duration::from_millis(150)); // simulated finalize/mux

        let dest = config.output_dir.join(format!("{}.mp4", config.file_stem));
        match fixture_path() {
            Some(fixture) => {
                fs::copy(fixture, &dest)?;
            }
            None => {
                // No sample fixture checked in — write a placeholder so
                // downstream code (library scan, DB row, review UI) has a
                // real file to point at.
                fs::write(&dest, b"NINJA_RECORDER_STUB_PLACEHOLDER")?;
            }
        }
        Ok(dest)
    }

    fn is_recording(&self) -> bool {
        self.active.is_some()
    }
}

fn fixture_path() -> Option<PathBuf> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("fixtures")
        .join("sample.mp4");
    path.exists().then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_then_stop_produces_a_file() {
        let dir = std::env::temp_dir().join(format!("ninja-recorder-test-{}", std::process::id()));
        let mut rec = StubRecorder::new();

        rec.start(RecordConfig {
            output_dir: dir.clone(),
            file_stem: "test".into(),
        })
        .unwrap();
        assert!(rec.is_recording());

        let path = rec.stop().unwrap();
        assert!(!rec.is_recording());
        assert!(path.exists());

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn stop_without_start_errors() {
        let mut rec = StubRecorder::new();
        assert!(matches!(rec.stop(), Err(RecorderError::NotRecording)));
    }

    #[test]
    fn double_start_errors() {
        let dir = std::env::temp_dir().join(format!("ninja-recorder-test2-{}", std::process::id()));
        let mut rec = StubRecorder::new();
        rec.start(RecordConfig {
            output_dir: dir.clone(),
            file_stem: "test".into(),
        })
        .unwrap();

        let second = rec.start(RecordConfig {
            output_dir: dir.clone(),
            file_stem: "test2".into(),
        });
        assert!(matches!(second, Err(RecorderError::AlreadyRecording)));

        rec.stop().unwrap();
        fs::remove_dir_all(&dir).ok();
    }
}
