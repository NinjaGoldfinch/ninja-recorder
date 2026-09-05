//! What gets captured, and which mp4 audio track it lands on.
//!
//! Deliberately libobs-free, like the rest of this module's public surface
//! (see the header of `recorder/mod.rs`): these types cross the Tauri command
//! boundary to the settings UI, get persisted into `settings_kv` and
//! `recordings.audio_tracks_json`, and are unit-tested on macOS where no
//! capture backend exists at all.
//!
//! **Track 0 is always the combined mix**, and every track after it is an
//! isolated stem. That ordering is the whole point: a VOD recorded today can
//! have the microphone cut out of it by a clip exporter written later, and a
//! player that knows nothing about any of this still plays the right thing by
//! picking the first track. See DEVELOPMENT.md §2.5.

use serde::{Deserialize, Serialize};

/// Discord's main executable. Process audio capture matches on the executable
/// rather than the window title because Discord retitles itself to whatever
/// channel you're looking at — see `AudioSourceKind::Application`.
pub const DISCORD_EXE: &str = "Discord.exe";

/// libobs' `MAX_AUDIO_MIXES`. No preset comes close (the largest is four),
/// but a `Custom` layout arrives from outside and has to be bounded somewhere.
pub const MAX_TRACKS: usize = 6;

/// One capturable audio source. Each maps to exactly one libobs source object
/// in the Windows backend; the mapping lives there, not here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AudioSourceKind {
    /// The League process' own audio, captured per-application.
    Game,
    /// The default output device, i.e. everything the machine is playing.
    Desktop,
    /// An input device. `None` means "whatever Windows calls default" —
    /// resolved by the capture backend, not here, because the answer can
    /// change between the settings screen and the game.
    Microphone {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        device_id: Option<String>,
    },
    /// Another application's audio, matched by executable name. Generic
    /// rather than a `Discord` variant so the eventual `Custom` preset can
    /// name Spotify or anything else without a new variant and a migration.
    Application { exe: String },
}

impl AudioSourceKind {
    /// Whether this source records the user's voice. Used to keep the
    /// promise that a preset which doesn't name a microphone never captures
    /// one — asserted in the tests below rather than left to review.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub fn is_microphone(&self) -> bool {
        matches!(self, Self::Microphone { .. })
    }
}

/// One mp4 audio track: a label for the UI and the set of sources mixed into
/// it, as indices into [`AudioLayout::sources`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioTrackSpec {
    pub label: String,
    pub sources: Vec<usize>,
}

/// A resolved capture plan. This is what gets persisted per recording, so it
/// carries `sources` alongside `tracks` — the indices in `AudioTrackSpec` are
/// meaningless without them, and a VOD has to stay readable long after the
/// preset that produced it was changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioLayout {
    pub sources: Vec<AudioSourceKind>,
    pub tracks: Vec<AudioTrackSpec>,
}

impl AudioLayout {
    /// Per-source libobs mixer bitmasks: bit *n* set means "feed track *n*".
    ///
    /// This is the one computation the whole multi-track design rests on. A
    /// libobs source can feed several mixers at once, so putting game audio
    /// on both the combined track and its own stem costs one extra bit
    /// rather than a second capture. Sources are *not* free to default here:
    /// libobs initialises `audio_mixers` to `0xFF` (every mix), so a source
    /// this returns `0` for must still be explicitly silenced by the backend
    /// or it leaks into every track.
    ///
    /// Consumed by the libobs backend, which only exists on Windows.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub fn mixer_masks(&self) -> Vec<u32> {
        let mut masks = vec![0u32; self.sources.len()];
        for (track_idx, track) in self.tracks.iter().enumerate() {
            for &source_idx in &track.sources {
                if let Some(mask) = masks.get_mut(source_idx) {
                    *mask |= 1 << track_idx;
                }
            }
        }
        masks
    }

    /// Rejects a layout that would produce a file the rest of the app can't
    /// describe. Only `Custom` can fail this — the built-in presets are
    /// covered by the tests below — but `Custom` is deserialized from a
    /// settings row a user could hand-edit.
    pub fn validate(&self) -> Result<(), String> {
        if self.tracks.is_empty() {
            return Err("an audio layout needs at least one track".into());
        }
        if self.tracks.len() > MAX_TRACKS {
            return Err(format!(
                "{} audio tracks requested, libobs supports at most {MAX_TRACKS}",
                self.tracks.len()
            ));
        }
        for (i, track) in self.tracks.iter().enumerate() {
            if track.sources.is_empty() {
                return Err(format!("audio track {i} ({}) has no sources", track.label));
            }
            for &source_idx in &track.sources {
                if source_idx >= self.sources.len() {
                    return Err(format!(
                        "audio track {i} ({}) references source {source_idx}, but only {} are defined",
                        track.label,
                        self.sources.len()
                    ));
                }
            }
        }
        Ok(())
    }
}

/// What the user picked in Settings. Persisted as JSON in `settings_kv` under
/// `audio_preset`; see `db::Db::get_audio_preset`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "preset", rename_all = "snake_case")]
pub enum AudioPreset {
    /// Game audio only. One track, not two: with a single source the
    /// combined mix and the stem are the same signal, so a second track
    /// would be a byte-identical duplicate.
    #[default]
    Game,
    GameMic {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mic_device_id: Option<String>,
    },
    GameMicDiscord {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mic_device_id: Option<String>,
    },
    /// Everything the machine is playing, with the game isolated onto track 1
    /// in case the mix turns out to contain something unwanted.
    Desktop,
    /// Not yet reachable from the UI — the settings screen writes only the
    /// four presets above. Representable so the storage format doesn't need
    /// to change when it is.
    Custom {
        sources: Vec<AudioSourceKind>,
        tracks: Vec<AudioTrackSpec>,
    },
    /// A preset written by a newer build. Without this, downgrading turns an
    /// unrecognised value into a hard deserialize error at startup instead of
    /// a quiet fall back to `Game`.
    #[serde(other)]
    Unknown,
}

impl AudioPreset {
    /// Expands a preset into the capture plan. Pure — no device enumeration,
    /// no clock, no I/O — so every preset's track layout is directly
    /// unit-tested below rather than only observable on a Windows box.
    pub fn layout(&self) -> AudioLayout {
        let track = |label: &str, sources: &[usize]| AudioTrackSpec {
            label: label.to_string(),
            sources: sources.to_vec(),
        };
        let mic = |device_id: &Option<String>| AudioSourceKind::Microphone {
            device_id: device_id.clone(),
        };

        match self {
            // `Unknown` deliberately shares the default's layout: a preset
            // this build can't read must still record something sensible.
            Self::Game | Self::Unknown => AudioLayout {
                sources: vec![AudioSourceKind::Game],
                tracks: vec![track("Game", &[0])],
            },
            Self::GameMic { mic_device_id } => AudioLayout {
                sources: vec![AudioSourceKind::Game, mic(mic_device_id)],
                tracks: vec![
                    track("Everything", &[0, 1]),
                    track("Game", &[0]),
                    track("Mic", &[1]),
                ],
            },
            Self::GameMicDiscord { mic_device_id } => AudioLayout {
                sources: vec![
                    AudioSourceKind::Game,
                    mic(mic_device_id),
                    AudioSourceKind::Application {
                        exe: DISCORD_EXE.to_string(),
                    },
                ],
                tracks: vec![
                    track("Everything", &[0, 1, 2]),
                    track("Game", &[0]),
                    track("Mic", &[1]),
                    track("Discord", &[2]),
                ],
            },
            // Desktop capture already contains the game, so the game source
            // feeds *only* its stem — adding it to track 0 as well would
            // mix the same audio in twice.
            Self::Desktop => AudioLayout {
                sources: vec![AudioSourceKind::Desktop, AudioSourceKind::Game],
                tracks: vec![track("System audio", &[0]), track("Game", &[1])],
            },
            Self::Custom { sources, tracks } => AudioLayout {
                sources: sources.clone(),
                tracks: tracks.clone(),
            },
        }
    }
}

/// An audio input device offered in the microphone picker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioInputDevice {
    /// The endpoint id the capture backend wants, verbatim.
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(layout: &AudioLayout) -> Vec<&str> {
        layout.tracks.iter().map(|t| t.label.as_str()).collect()
    }

    #[test]
    fn game_is_a_single_track() {
        let layout = AudioPreset::Game.layout();
        assert_eq!(labels(&layout), ["Game"]);
        assert_eq!(layout.sources, vec![AudioSourceKind::Game]);
    }

    #[test]
    fn desktop_puts_system_audio_first_and_isolates_the_game() {
        let layout = AudioPreset::Desktop.layout();
        assert_eq!(labels(&layout), ["System audio", "Game"]);
        // The game must not also feed track 0 — desktop capture already
        // contains it, and mixing it in twice would double the level.
        assert_eq!(layout.tracks[0].sources, vec![0]);
        assert_eq!(layout.sources[0], AudioSourceKind::Desktop);
        assert_eq!(layout.tracks[1].sources, vec![1]);
        assert_eq!(layout.sources[1], AudioSourceKind::Game);
    }

    #[test]
    fn game_mic_is_combined_then_stems() {
        let layout = AudioPreset::GameMic {
            mic_device_id: None,
        }
        .layout();
        assert_eq!(labels(&layout), ["Everything", "Game", "Mic"]);
        assert_eq!(layout.tracks[0].sources, vec![0, 1]);
    }

    #[test]
    fn game_mic_discord_mixes_every_source_into_track_zero() {
        let layout = AudioPreset::GameMicDiscord {
            mic_device_id: None,
        }
        .layout();
        assert_eq!(labels(&layout), ["Everything", "Game", "Mic", "Discord"]);

        let combined = &layout.tracks[0].sources;
        for source_idx in 0..layout.sources.len() {
            assert!(
                combined.contains(&source_idx),
                "source {source_idx} ({:?}) is missing from the combined track",
                layout.sources[source_idx]
            );
        }
    }

    /// The promise the settings screen makes to the user: a preset that
    /// doesn't name a microphone never captures one.
    #[test]
    fn only_presets_that_name_a_mic_capture_one() {
        for preset in [AudioPreset::Game, AudioPreset::Desktop, AudioPreset::Unknown] {
            let layout = preset.layout();
            assert!(
                !layout.sources.iter().any(AudioSourceKind::is_microphone),
                "{preset:?} captured a microphone"
            );
        }
        for preset in [
            AudioPreset::GameMic {
                mic_device_id: None,
            },
            AudioPreset::GameMicDiscord {
                mic_device_id: None,
            },
        ] {
            assert!(preset
                .layout()
                .sources
                .iter()
                .any(AudioSourceKind::is_microphone));
        }
    }

    #[test]
    fn mixer_masks_put_each_source_on_the_combined_track_and_its_own_stem() {
        let layout = AudioPreset::GameMicDiscord {
            mic_device_id: None,
        }
        .layout();
        assert_eq!(
            layout.mixer_masks(),
            vec![
                0b0011, // game    -> combined + stem 1
                0b0101, // mic     -> combined + stem 2
                0b1001, // discord -> combined + stem 3
            ]
        );
    }

    #[test]
    fn desktop_mixer_masks_keep_the_game_off_the_combined_track() {
        let layout = AudioPreset::Desktop.layout();
        assert_eq!(layout.mixer_masks(), vec![0b01, 0b10]);
    }

    #[test]
    fn every_preset_produces_a_valid_dense_layout() {
        for preset in [
            AudioPreset::Game,
            AudioPreset::GameMic {
                mic_device_id: None,
            },
            AudioPreset::GameMicDiscord {
                mic_device_id: None,
            },
            AudioPreset::Desktop,
            AudioPreset::Unknown,
        ] {
            let layout = preset.layout();
            layout.validate().unwrap_or_else(|e| panic!("{preset:?}: {e}"));

            // libobs stops at the first unbound output track, so a hole in
            // the track list silently truncates the file to one track.
            for (i, mask) in layout.mixer_masks().iter().enumerate() {
                assert_ne!(*mask, 0, "{preset:?}: source {i} feeds no track");
            }
            for track_idx in 0..layout.tracks.len() {
                assert!(
                    layout
                        .mixer_masks()
                        .iter()
                        .any(|m| m & (1 << track_idx) != 0),
                    "{preset:?}: track {track_idx} has no source"
                );
            }
        }
    }

    #[test]
    fn validate_rejects_an_out_of_range_source_index() {
        let layout = AudioLayout {
            sources: vec![AudioSourceKind::Game],
            tracks: vec![AudioTrackSpec {
                label: "Nope".into(),
                sources: vec![4],
            }],
        };
        assert!(layout.validate().is_err());
    }

    #[test]
    fn validate_rejects_more_tracks_than_libobs_has_mixes() {
        let layout = AudioLayout {
            sources: vec![AudioSourceKind::Game],
            tracks: (0..MAX_TRACKS + 1)
                .map(|i| AudioTrackSpec {
                    label: format!("t{i}"),
                    sources: vec![0],
                })
                .collect(),
        };
        assert!(layout.validate().is_err());
    }

    #[test]
    fn preset_round_trips_through_json() {
        let preset = AudioPreset::GameMic {
            mic_device_id: Some("{0.0.1.00000000}.{abc}".into()),
        };
        let json = serde_json::to_string(&preset).unwrap();
        assert_eq!(
            serde_json::from_str::<AudioPreset>(&json).unwrap(),
            preset,
            "{json}"
        );
    }

    #[test]
    fn a_mic_less_preset_serializes_without_a_null_device() {
        let json = serde_json::to_string(&AudioPreset::Game).unwrap();
        assert_eq!(json, r#"{"preset":"game"}"#);
    }

    /// The reason `Unknown` exists: installing an older build over a newer
    /// one must not fail to read its own settings row.
    #[test]
    fn an_unrecognised_preset_reads_back_as_unknown_not_an_error() {
        let parsed: AudioPreset =
            serde_json::from_str(r#"{"preset":"game_mic_spotify"}"#).unwrap();
        assert_eq!(parsed, AudioPreset::Unknown);
        assert_eq!(parsed.layout(), AudioPreset::Game.layout());
    }

    #[test]
    fn a_custom_layout_round_trips() {
        let preset = AudioPreset::Custom {
            sources: vec![
                AudioSourceKind::Game,
                AudioSourceKind::Application {
                    exe: "Spotify.exe".into(),
                },
            ],
            tracks: vec![
                AudioTrackSpec {
                    label: "Everything".into(),
                    sources: vec![0, 1],
                },
                AudioTrackSpec {
                    label: "Game".into(),
                    sources: vec![0],
                },
            ],
        };
        let json = serde_json::to_string(&preset).unwrap();
        assert_eq!(serde_json::from_str::<AudioPreset>(&json).unwrap(), preset);
        preset.layout().validate().unwrap();
    }
}
