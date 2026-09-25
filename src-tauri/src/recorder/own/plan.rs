//! What a recording captures, and what the file it writes then holds, with no
//! Windows in it.
//!
//! [`plan`] turns an [`AudioLayout`] (a preset, expanded by
//! `AudioPreset::layout`) into a [`CapturePlan`]: the sources to open, each
//! once, and for every track the file will have, which of them sum into it.
//! [`realised_layout`] then answers the other half, once the sources have
//! been opened: a source that could not open (no microphone, Discord not
//! running) is dropped along with any track it alone fed, and the rest are
//! reindexed, so the layout `stop` reports describes the file that exists
//! rather than the one that was asked for (DEVELOPMENT.md §2.5).
//!
//! **Every track is written since #239**: track 0, the combined mix, and each
//! stem after it, one AAC encoder apiece, into one file through our own MP4
//! writer (`own::mux`). #238 wrote track 0 only, because Media Foundation's
//! sink writer holds one audio stream; [`TRACKS_WRITTEN`] was 1 then.
//! [`describe`] is how the log names what a file holds.

use crate::recorder::audio::{AudioLayout, AudioSourceKind, AudioTrackSpec};

/// How many of a layout's tracks the own backend writes: all of them.
/// [`plan`] takes at most this many, so any layout is planned whole.
pub const TRACKS_WRITTEN: usize = usize::MAX;

/// The sources a recording opens, and what each written track sums.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapturePlan {
    /// Every source to open, each once, in the order the layout first names
    /// them. Only sources that feed a written track are here, which since
    /// #239 is every source the layout names: the Desktop preset's game
    /// source feeds only its stem, and is opened for it.
    pub sources: Vec<AudioSourceKind>,
    /// The written tracks, in file order. Each track's `sources` index
    /// [`CapturePlan::sources`], without repeats.
    pub tracks: Vec<AudioTrackSpec>,
}

impl CapturePlan {
    /// The layout the file holds if every source opens.
    pub fn layout(&self) -> AudioLayout {
        AudioLayout { sources: self.sources.clone(), tracks: self.tracks.clone() }
    }
}

/// Whether two sources are the same capture. An application is matched by
/// executable name the way Windows compares them, without regard to case.
fn same_source(a: &AudioSourceKind, b: &AudioSourceKind) -> bool {
    match (a, b) {
        (AudioSourceKind::Application { exe: a }, AudioSourceKind::Application { exe: b }) => {
            a.eq_ignore_ascii_case(b)
        }
        _ => a == b,
    }
}

/// The plan for the first `tracks` tracks of `layout`.
///
/// `layout` is assumed valid (`AudioLayout::validate`, which
/// `select::audio_layout` runs); an index past its sources is skipped rather
/// than trusted. A source named twice (only a hand-edited `Custom` layout can)
/// is opened once, so it is never captured twice and summed in at double
/// level; for the same reason a track naming one source twice sums it once.
pub fn plan(layout: &AudioLayout, tracks: usize) -> CapturePlan {
    let mut sources: Vec<AudioSourceKind> = Vec::new();
    let mut planned = Vec::new();
    for track in layout.tracks.iter().take(tracks) {
        let mut summed: Vec<usize> = Vec::new();
        for kind in track.sources.iter().filter_map(|&i| layout.sources.get(i)) {
            let index = match sources.iter().position(|s| same_source(s, kind)) {
                Some(index) => index,
                None => {
                    sources.push(kind.clone());
                    sources.len() - 1
                }
            };
            if !summed.contains(&index) {
                summed.push(index);
            }
        }
        planned.push(AudioTrackSpec { label: track.label.clone(), sources: summed });
    }
    CapturePlan { sources, tracks: planned }
}

/// `layout` as it is once the sources have been opened: `opened[i]` says
/// whether source `i` did. A source that did not is removed from every track;
/// a track left with no source at all (the failed source's stem) is dropped;
/// and the survivors are reindexed. A source missing from `opened` counts as
/// not opened.
///
/// Every source dropped means an empty layout, which is a video-only file,
/// and says so.
pub fn realised_layout(layout: &AudioLayout, opened: &[bool]) -> AudioLayout {
    let is_open = |i: usize| opened.get(i).copied().unwrap_or(false);
    let mut new_index = vec![None; layout.sources.len()];
    let mut sources = Vec::new();
    for (i, kind) in layout.sources.iter().enumerate() {
        if is_open(i) {
            new_index[i] = Some(sources.len());
            sources.push(kind.clone());
        }
    }
    let tracks = layout
        .tracks
        .iter()
        .filter_map(|track| {
            let summed: Vec<usize> =
                track.sources.iter().filter_map(|&i| new_index.get(i).copied().flatten()).collect();
            (!summed.is_empty())
                .then(|| AudioTrackSpec { label: track.label.clone(), sources: summed })
        })
        .collect();
    AudioLayout { sources, tracks }
}

/// What a source is called in the log, and its capture thread's name:
/// `game`, `microphone`, `desktop`, or the application's executable.
pub fn source_name(kind: &AudioSourceKind) -> String {
    match kind {
        AudioSourceKind::Game => "game".to_string(),
        AudioSourceKind::Microphone { .. } => "microphone".to_string(),
        AudioSourceKind::Desktop => "desktop".to_string(),
        AudioSourceKind::Application { exe } => exe.clone(),
    }
}

/// Every track of `layout` as the log names it, in file order: `a:0
/// "Everything" (game + microphone + Discord.exe); a:1 "Game" (game)`. Track
/// `a:N` is the file's audio stream N, which is how ffprobe and the review
/// player count them; `a:0` is the default. `no audio tracks` for a
/// video-only layout.
pub fn describe(layout: &AudioLayout) -> String {
    if layout.tracks.is_empty() {
        return "no audio tracks".to_string();
    }
    let tracks: Vec<String> = layout
        .tracks
        .iter()
        .enumerate()
        .map(|(i, track)| {
            let names: Vec<String> = track
                .sources
                .iter()
                .map(|&s| layout.sources.get(s).map_or_else(|| format!("source {s}"), source_name))
                .collect();
            format!("a:{i} \"{}\" ({})", track.label, names.join(" + "))
        })
        .collect();
    tracks.join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recorder::audio::{AudioPreset, DISCORD_EXE};

    fn mic() -> AudioSourceKind {
        AudioSourceKind::Microphone { device_id: None }
    }

    fn discord() -> AudioSourceKind {
        AudioSourceKind::Application { exe: DISCORD_EXE.to_string() }
    }

    fn track(label: &str, sources: &[usize]) -> AudioTrackSpec {
        AudioTrackSpec { label: label.to_string(), sources: sources.to_vec() }
    }

    fn every_preset() -> Vec<AudioPreset> {
        vec![
            AudioPreset::Game,
            AudioPreset::GameMic { mic_device_id: None },
            AudioPreset::GameMicDiscord { mic_device_id: None },
            AudioPreset::Desktop,
            AudioPreset::Unknown,
        ]
    }

    /// The whole plan of every preset is `audio.rs`'s table itself: the
    /// presets name each source once, so nothing is merged or moved.
    #[test]
    fn the_whole_plan_of_every_preset_is_its_layout() {
        for preset in every_preset() {
            let layout = preset.layout();
            let whole = plan(&layout, layout.tracks.len());
            assert_eq!(whole.layout(), layout, "{preset:?}");
        }
    }

    /// What the backend writes since #239: every track of every preset, so
    /// the plan is the layout itself.
    #[test]
    fn every_track_is_written() {
        for preset in every_preset() {
            let layout = preset.layout();
            assert_eq!(plan(&layout, TRACKS_WRITTEN).layout(), layout, "{preset:?}");
        }
    }

    /// Track 0 alone, as #238 wrote it: only the sources the mix sums.
    #[test]
    fn the_mix_plan_of_every_preset_is_track_0() {
        let cases = [
            (AudioPreset::Game, vec![AudioSourceKind::Game], "Game"),
            (AudioPreset::GameMic { mic_device_id: None }, vec![AudioSourceKind::Game, mic()], "Everything"),
            (
                AudioPreset::GameMicDiscord { mic_device_id: None },
                vec![AudioSourceKind::Game, mic(), discord()],
                "Everything",
            ),
            (AudioPreset::Desktop, vec![AudioSourceKind::Desktop], "System audio"),
            (AudioPreset::Unknown, vec![AudioSourceKind::Game], "Game"),
        ];
        for (preset, sources, label) in cases {
            let mix = plan(&preset.layout(), 1);
            assert_eq!(mix.sources, sources, "{preset:?}");
            let every: Vec<usize> = (0..sources.len()).collect();
            assert_eq!(mix.tracks, vec![track(label, &every)], "{preset:?}");
        }
    }

    /// The Desktop preset's mix is the desktop alone. Desktop capture already
    /// contains the game, so summing the game in as well would play it twice
    /// (the same promise `audio.rs`'s Desktop test makes of the layout).
    #[test]
    fn the_desktop_mix_never_has_the_game_twice() {
        let mix = plan(&AudioPreset::Desktop.layout(), 1);
        assert_eq!(mix.sources, vec![AudioSourceKind::Desktop]);
        assert_eq!(mix.tracks[0].sources, vec![0]);
        assert!(!mix.sources.contains(&AudioSourceKind::Game));
        // And with its stem, the game is opened for track 1 only.
        let whole = plan(&AudioPreset::Desktop.layout(), 2);
        assert_eq!(whole.tracks[0].sources, vec![0]);
        assert_eq!(whole.tracks[1].sources, vec![1]);
        assert_eq!(whole.sources[1], AudioSourceKind::Game);
    }

    /// The mic preset opens the configured device, not the default.
    #[test]
    fn a_configured_microphone_is_carried_into_the_plan() {
        let id = "{0.0.1.00000000}.{abc}".to_string();
        let mix = plan(&AudioPreset::GameMic { mic_device_id: Some(id.clone()) }.layout(), 1);
        assert_eq!(mix.sources[1], AudioSourceKind::Microphone { device_id: Some(id) });
    }

    #[test]
    fn a_source_named_twice_is_opened_once_and_summed_once() {
        let layout = AudioLayout {
            sources: vec![
                AudioSourceKind::Game,
                AudioSourceKind::Application { exe: "discord.EXE".into() },
                AudioSourceKind::Game,
                discord(),
            ],
            tracks: vec![track("All", &[0, 1, 2, 3, 0]), track("Discord", &[3])],
        };
        let whole = plan(&layout, 2);
        assert_eq!(whole.sources.len(), 2, "{whole:?}");
        assert_eq!(whole.tracks, vec![track("All", &[0, 1]), track("Discord", &[1])]);
    }

    #[test]
    fn an_index_past_the_sources_is_skipped() {
        let layout =
            AudioLayout { sources: vec![AudioSourceKind::Game], tracks: vec![track("Game", &[0, 7])] };
        assert_eq!(plan(&layout, 1).tracks, vec![track("Game", &[0])]);
    }

    #[test]
    fn every_source_opening_changes_nothing() {
        for preset in every_preset() {
            let layout = preset.layout();
            let all = vec![true; layout.sources.len()];
            assert_eq!(realised_layout(&layout, &all), layout, "{preset:?}");
        }
    }

    /// No microphone: the mic leaves the mix and its stem goes, and Discord,
    /// which was source 2, is source 1 in what `stop` reports.
    #[test]
    fn a_failed_source_drops_its_stem_and_the_rest_are_reindexed() {
        let layout = AudioPreset::GameMicDiscord { mic_device_id: None }.layout();
        let realised = realised_layout(&layout, &[true, false, true]);
        assert_eq!(realised.sources, vec![AudioSourceKind::Game, discord()]);
        assert_eq!(
            realised.tracks,
            vec![track("Everything", &[0, 1]), track("Game", &[0]), track("Discord", &[1])]
        );
        realised.validate().unwrap();

        // Discord not running, with the mix only: two sources, one track.
        let mix = plan(&layout, 1).layout();
        let realised = realised_layout(&mix, &[true, true, false]);
        assert_eq!(realised.sources, vec![AudioSourceKind::Game, mic()]);
        assert_eq!(realised.tracks, vec![track("Everything", &[0, 1])]);
    }

    /// The mix survives while any of its sources did; only when every one
    /// failed does it go, and a file with no audio reports no tracks.
    #[test]
    fn nothing_opening_is_a_video_only_layout() {
        let layout = AudioPreset::GameMic { mic_device_id: None }.layout();
        let game_only = realised_layout(&layout, &[true, false]);
        assert_eq!(game_only.tracks, vec![track("Everything", &[0]), track("Game", &[0])]);

        let none = realised_layout(&layout, &[false, false]);
        assert!(none.sources.is_empty() && none.tracks.is_empty(), "{none:?}");
        // A short `opened` is read as not opened, never as out of bounds.
        assert_eq!(realised_layout(&layout, &[]), none);
    }

    /// Desktop with the desktop failed: its track goes, and the game's stem is
    /// what is left.
    #[test]
    fn a_failed_mix_source_leaves_the_stems_in_order() {
        let layout = AudioPreset::Desktop.layout();
        let realised = realised_layout(&layout, &[false, true]);
        assert_eq!(realised.sources, vec![AudioSourceKind::Game]);
        assert_eq!(realised.tracks, vec![track("Game", &[0])]);
    }

    /// What `start` logs for the file, for every preset: each track in file
    /// order with its label and the sources it sums, `a:0` first.
    #[test]
    fn every_track_is_named_with_its_label_and_sources() {
        let cases = [
            (AudioPreset::Game, r#"a:0 "Game" (game)"#),
            (
                AudioPreset::GameMic { mic_device_id: None },
                r#"a:0 "Everything" (game + microphone); a:1 "Game" (game); a:2 "Mic" (microphone)"#,
            ),
            (
                AudioPreset::GameMicDiscord { mic_device_id: None },
                r#"a:0 "Everything" (game + microphone + Discord.exe); a:1 "Game" (game); a:2 "Mic" (microphone); a:3 "Discord" (Discord.exe)"#,
            ),
            (AudioPreset::Desktop, r#"a:0 "System audio" (desktop); a:1 "Game" (game)"#),
        ];
        for (preset, want) in cases {
            assert_eq!(describe(&preset.layout()), want, "{preset:?}");
        }
        let empty = AudioLayout { sources: vec![], tracks: vec![] };
        assert_eq!(describe(&empty), "no audio tracks");
    }

    /// What `stop` reports after a failed microphone is described the same
    /// way: the stems that exist, renumbered.
    #[test]
    fn a_realised_layout_is_described_as_the_file_holds_it() {
        let layout = AudioPreset::GameMicDiscord { mic_device_id: None }.layout();
        let realised = realised_layout(&layout, &[true, false, true]);
        assert_eq!(
            describe(&realised),
            r#"a:0 "Everything" (game + Discord.exe); a:1 "Game" (game); a:2 "Discord" (Discord.exe)"#
        );
        assert_eq!(source_name(&AudioSourceKind::Microphone { device_id: Some("x".into()) }), "microphone");
    }
}
