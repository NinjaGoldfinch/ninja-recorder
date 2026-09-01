//! Live Client Data types, marker extraction, and time alignment.
//! DEVELOPMENT.md §3.2, §3.4, §4.
//!
//! Marker classification (`classify_event`) and `MarkerTracker` are pure/
//! stateful-but-sync, so they're fully testable against fixture JSON
//! without a live poller — see the tests module and
//! `fixtures/live-client/sample-allgamedata.json`.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

// --- Live Client Data response shape (subset we care about) ------------

#[derive(Debug, Clone, Deserialize)]
pub struct AllGameData {
    #[serde(rename = "activePlayer")]
    pub active_player: Option<ActivePlayer>,
    // `allPlayers` (team, champion, etc. for every player) is part of the
    // real response but unused here — marker extraction only needs to
    // know our own name, matched directly against event Killer/Victim/
    // Assister fields (see `classify_event`). Not modeled to avoid an
    // unused struct; add it back if a future feature needs per-team
    // classification.
    pub events: EventsWrapper,
    #[serde(rename = "gameData")]
    pub game_data: GameData,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ActivePlayer {
    #[serde(rename = "summonerName", default)]
    pub summoner_name: String,
    #[serde(rename = "riotIdGameName", default)]
    pub riot_id_game_name: String,
}

impl ActivePlayer {
    /// Resolves whichever display-name field this API version populates:
    /// `riotIdGameName` after the Riot ID rollout, `summonerName` on older
    /// clients. Prefers Riot ID when both are present.
    pub fn display_name(&self) -> &str {
        if !self.riot_id_game_name.is_empty() {
            &self.riot_id_game_name
        } else {
            &self.summoner_name
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct EventsWrapper {
    #[serde(rename = "Events", default)]
    pub events: Vec<GameEvent>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GameEvent {
    #[serde(rename = "EventID")]
    pub event_id: i64,
    #[serde(rename = "EventName")]
    pub event_name: String,
    #[serde(rename = "EventTime")]
    pub event_time: f64,
    #[serde(rename = "KillerName", default)]
    pub killer_name: Option<String>,
    #[serde(rename = "VictimName", default)]
    pub victim_name: Option<String>,
    #[serde(rename = "Assisters", default)]
    pub assisters: Vec<String>,
    #[serde(rename = "Recipient", default)]
    pub recipient: Option<String>,
    #[serde(rename = "Acer", default)]
    pub acer: Option<String>,
    #[serde(rename = "AcingTeam", default)]
    pub acing_team: Option<String>,
    #[serde(rename = "DragonType", default)]
    pub dragon_type: Option<String>,
    #[serde(rename = "TurretKilled", default)]
    pub turret_killed: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GameData {
    #[serde(rename = "gameTime")]
    pub game_time: f64,
}

// --- Markers -------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MarkerKind {
    Kill,
    Death,
    Assist,
    Dragon,
    Baron,
    Herald,
    Turret,
    Ace,
    FirstBlood,
}

impl MarkerKind {
    /// Matches both the DB `markers.kind` values (DEVELOPMENT.md §4) and
    /// this enum's own `snake_case` serde representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            MarkerKind::Kill => "kill",
            MarkerKind::Death => "death",
            MarkerKind::Assist => "assist",
            MarkerKind::Dragon => "dragon",
            MarkerKind::Baron => "baron",
            MarkerKind::Herald => "herald",
            MarkerKind::Turret => "turret",
            MarkerKind::Ace => "ace",
            MarkerKind::FirstBlood => "first_blood",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Marker {
    pub kind: MarkerKind,
    pub game_time_s: f64,
    /// Structured detail specific to the marker kind (killer/victim/dragon
    /// type/etc.) — matches the `payload_json` column planned in
    /// DEVELOPMENT.md §4 so this serializes straight into the DB in Phase 4.
    pub payload: serde_json::Value,
}

/// Classifies one event into a marker, or `None` if it's not a kind we
/// track (`GameStart`, `MinionsSpawning`, etc.). Only `ChampionKill` is
/// filtered to events involving us (kill/death/assist) — every other kind
/// is recorded regardless of which team it belongs to, since seeing what
/// the *enemy* team did (e.g. they took Baron while we were dead) is
/// useful VOD-review context.
fn classify_event(event: &GameEvent, our_name: Option<&str>) -> Option<Marker> {
    let is_ours = |name: &Option<String>| -> bool {
        match (name, our_name) {
            (Some(n), Some(us)) => names_match(n, us),
            _ => false,
        }
    };

    match event.event_name.as_str() {
        "ChampionKill" => {
            if is_ours(&event.killer_name) {
                Some(Marker {
                    kind: MarkerKind::Kill,
                    game_time_s: event.event_time,
                    payload: serde_json::json!({ "victim": event.victim_name }),
                })
            } else if is_ours(&event.victim_name) {
                Some(Marker {
                    kind: MarkerKind::Death,
                    game_time_s: event.event_time,
                    payload: serde_json::json!({ "killer": event.killer_name }),
                })
            } else if our_name
                .map(|us| event.assisters.iter().any(|a| names_match(a, us)))
                .unwrap_or(false)
            {
                Some(Marker {
                    kind: MarkerKind::Assist,
                    game_time_s: event.event_time,
                    payload: serde_json::json!({
                        "victim": event.victim_name,
                        "killer": event.killer_name,
                    }),
                })
            } else {
                None
            }
        }
        "TurretKilled" => Some(Marker {
            kind: MarkerKind::Turret,
            game_time_s: event.event_time,
            payload: serde_json::json!({
                "killer": event.killer_name,
                "turret": event.turret_killed,
            }),
        }),
        "DragonKill" => Some(Marker {
            kind: MarkerKind::Dragon,
            game_time_s: event.event_time,
            payload: serde_json::json!({
                "killer": event.killer_name,
                "dragon_type": event.dragon_type,
            }),
        }),
        "BaronKill" => Some(Marker {
            kind: MarkerKind::Baron,
            game_time_s: event.event_time,
            payload: serde_json::json!({ "killer": event.killer_name }),
        }),
        "HeraldKill" => Some(Marker {
            kind: MarkerKind::Herald,
            game_time_s: event.event_time,
            payload: serde_json::json!({ "killer": event.killer_name }),
        }),
        "Ace" => Some(Marker {
            kind: MarkerKind::Ace,
            game_time_s: event.event_time,
            payload: serde_json::json!({
                "acer": event.acer,
                "acing_team": event.acing_team,
            }),
        }),
        "FirstBlood" => Some(Marker {
            kind: MarkerKind::FirstBlood,
            game_time_s: event.event_time,
            payload: serde_json::json!({ "recipient": event.recipient }),
        }),
        _ => None,
    }
}

/// Compares display names leniently: case-insensitive, and ignoring a
/// `#tagline` suffix if only one side has it. The Live Client Data API's
/// exact name format (bare summoner name vs. `Name#TAG`) across event
/// fields vs. `activePlayer`/`allPlayers` hasn't been confirmed against a
/// live client — this tolerates either without needing to know which.
fn names_match(a: &str, b: &str) -> bool {
    fn base(s: &str) -> String {
        s.split('#').next().unwrap_or(s).trim().to_lowercase()
    }
    !a.is_empty() && !b.is_empty() && base(a) == base(b)
}

/// De-duplicates markers across repeated polls of the same game — each
/// poll returns the *entire* event list so far, not just what's new.
#[derive(Debug, Default)]
pub struct MarkerTracker {
    seen_event_ids: HashSet<i64>,
}

impl MarkerTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns only markers for events not already seen by this tracker.
    pub fn ingest(&mut self, snapshot: &AllGameData) -> Vec<Marker> {
        let mut fresh = Vec::new();
        for event in &snapshot.events.events {
            if !self.seen_event_ids.insert(event.event_id) {
                continue;
            }
            if let Some(marker) = classify_event(event, snapshot.active_player.as_ref().map(|p| p.display_name())) {
                fresh.push(marker);
            }
        }
        fresh
    }
}

// --- Time alignment --------------------------------------------------

/// Maps in-game time (seconds since `GameStart`) to video time (seconds
/// into the recording). DEVELOPMENT.md §3.2: recording starts on the
/// loading screen, before `gameTime` reaches 0, so early-game markers need
/// an offset rather than a direct 1:1 mapping.
#[derive(Debug, Clone, Copy)]
pub struct TimeAlignment {
    offset_s: f64,
}

impl TimeAlignment {
    /// `first_game_time_s` is the `gameTime` seen on the first successful
    /// poll after recording started; `elapsed_since_record_start_s` is how
    /// long recording had already been running at that poll (typically a
    /// few seconds — poll interval plus encoder spin-up).
    pub fn new(first_game_time_s: f64, elapsed_since_record_start_s: f64) -> Self {
        Self {
            offset_s: elapsed_since_record_start_s - first_game_time_s,
        }
    }

    /// Video-time position for a marker recorded at `game_time_s`. Clamped
    /// to 0 — a marker computed to land before the recording started (e.g.
    /// a backdated event right at game start) snaps to the beginning
    /// rather than producing a nonsensical negative seek target.
    pub fn video_time_s(&self, game_time_s: f64) -> f64 {
        (game_time_s + self.offset_s).max(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn as_str_matches_serde_snake_case_representation() {
        for kind in [
            MarkerKind::Kill,
            MarkerKind::Death,
            MarkerKind::Assist,
            MarkerKind::Dragon,
            MarkerKind::Baron,
            MarkerKind::Herald,
            MarkerKind::Turret,
            MarkerKind::Ace,
            MarkerKind::FirstBlood,
        ] {
            let serialized: String = serde_json::to_string(&kind).unwrap();
            assert_eq!(serialized, format!("\"{}\"", kind.as_str()));
        }
    }

    fn fixture() -> AllGameData {
        let json = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../fixtures/live-client/sample-allgamedata.json"
        ));
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn extracts_our_kill_death_and_assist_but_not_others_kill() {
        let markers = MarkerTracker::new().ingest(&fixture());

        let kinds: Vec<MarkerKind> = markers.iter().map(|m| m.kind).collect();
        assert!(kinds.contains(&MarkerKind::Kill));
        assert!(kinds.contains(&MarkerKind::Death));
        assert!(kinds.contains(&MarkerKind::Assist));

        // The fixture includes a ChampionKill between two other players
        // that doesn't involve us at all — it must not produce a marker.
        let kill_markers: Vec<_> = markers.iter().filter(|m| m.kind == MarkerKind::Kill).collect();
        assert_eq!(kill_markers.len(), 1, "only our own kill should produce a Kill marker");
    }

    #[test]
    fn extracts_all_objective_events_regardless_of_team() {
        let markers = MarkerTracker::new().ingest(&fixture());
        let kinds: Vec<MarkerKind> = markers.iter().map(|m| m.kind).collect();
        assert!(kinds.contains(&MarkerKind::Turret));
        assert!(kinds.contains(&MarkerKind::Dragon));
        assert!(kinds.contains(&MarkerKind::Baron));
        assert!(kinds.contains(&MarkerKind::Herald));
        assert!(kinds.contains(&MarkerKind::Ace));
        assert!(kinds.contains(&MarkerKind::FirstBlood));
    }

    #[test]
    fn ignores_events_that_are_not_in_the_extraction_list() {
        // Fixture includes GameStart / MinionsSpawning — neither should
        // produce a marker.
        let markers = MarkerTracker::new().ingest(&fixture());
        assert!(markers.len() < fixture().events.events.len());
    }

    #[test]
    fn names_match_is_case_insensitive_and_tagline_tolerant() {
        assert!(names_match("Ninja", "ninja"));
        assert!(names_match("Ninja#NA1", "Ninja"));
        assert!(names_match("Ninja", "Ninja#NA1"));
        assert!(!names_match("Ninja", "NotNinja"));
        assert!(!names_match("", "Ninja"));
    }

    #[test]
    fn marker_tracker_dedupes_across_repeated_polls() {
        let mut tracker = MarkerTracker::new();
        let snapshot = fixture();

        let first = tracker.ingest(&snapshot);
        assert!(!first.is_empty());

        // Same snapshot polled again (as happens every tick) — nothing new.
        let second = tracker.ingest(&snapshot);
        assert!(second.is_empty());
    }

    #[test]
    fn time_alignment_offsets_for_loading_screen() {
        // Recording had been running 8s (loading screen) when gameTime
        // first read 0.5s.
        let alignment = TimeAlignment::new(0.5, 8.0);
        assert!((alignment.video_time_s(0.5) - 8.0).abs() < f64::EPSILON);
        assert!((alignment.video_time_s(10.5) - 18.0).abs() < f64::EPSILON);
    }

    #[test]
    fn time_alignment_never_returns_negative() {
        let alignment = TimeAlignment::new(5.0, 0.0);
        assert_eq!(alignment.video_time_s(0.0), 0.0);
    }
}
