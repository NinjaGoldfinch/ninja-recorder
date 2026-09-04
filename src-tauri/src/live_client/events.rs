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
    /// Every player in the game, both teams. Marker extraction doesn't need
    /// this (it matches our own name straight against event Killer/Victim/
    /// Assister fields — see `classify_event`), but the review timeline's
    /// advantage curve does: it's the only place the API exposes per-player
    /// items and scores, and the only way to learn which side we're on.
    #[serde(rename = "allPlayers", default)]
    pub all_players: Vec<PlayerEntry>,
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
    /// *Unspent* gold, not gold earned — the only gold figure the Live
    /// Client Data API exposes, and only for us. See `team_diff`.
    #[serde(rename = "currentGold", default)]
    pub current_gold: f64,
    #[serde(default)]
    pub level: i64,
}

/// One entry from `allPlayers`. Every field is `default` because the
/// hand-trimmed fixtures omit most of them, and because a live response
/// that drops `items` for enemies must degrade to a zero contribution
/// rather than failing the whole poll (and with it, marker extraction).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PlayerEntry {
    #[serde(rename = "summonerName", default)]
    pub summoner_name: String,
    #[serde(rename = "riotIdGameName", default)]
    pub riot_id_game_name: String,
    /// "ORDER" (blue side) or "CHAOS" (red side).
    #[serde(default)]
    pub team: String,
    #[serde(default)]
    pub items: Vec<PlayerItem>,
    #[serde(default)]
    pub scores: PlayerScores,
}

impl PlayerEntry {
    /// Mirrors `ActivePlayer::candidate_names` — the same name ambiguity
    /// applies when matching an `allPlayers` entry back to us.
    fn candidate_names(&self) -> Vec<&str> {
        [self.summoner_name.as_str(), self.riot_id_game_name.as_str()]
            .into_iter()
            .filter(|n| !n.is_empty())
            .collect()
    }

    fn item_gold(&self) -> f64 {
        self.items
            .iter()
            .map(|i| (i.price * i.count.max(1)) as f64)
            .sum()
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct PlayerItem {
    #[serde(default)]
    pub price: i64,
    #[serde(default)]
    pub count: i64,
}

/// Only the two scores the advantage curve plots. `deaths`/`assists`/
/// `wardScore` are in the real response too, but modelling fields nothing
/// reads is what the original `allPlayers` comment was avoiding — add them
/// alongside a feature that needs them.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PlayerScores {
    #[serde(default)]
    pub kills: i64,
    #[serde(rename = "creepScore", default)]
    pub creep_score: i64,
}

impl ActivePlayer {
    /// Every name this player might be referred to by in event
    /// Killer/Victim/Assister fields. Confirmed via a live capture
    /// (Practice Tool, 2026-09-01) that those fields use `summonerName`
    /// even when `riotIdGameName` is populated too — a Practice Tool
    /// summoner name happened to be the champion name ("Ahri"), and the
    /// real ChampionKill events used exactly that, not the Riot ID game
    /// name ("NinjaGoldfinch"). Matching against every non-empty
    /// candidate rather than picking one is the robust fix: a real
    /// (non-Practice-Tool) game hasn't been confirmed to behave the same
    /// way, and this way it doesn't matter which one the client actually
    /// uses in any given match type.
    pub fn candidate_names(&self) -> Vec<&str> {
        [self.summoner_name.as_str(), self.riot_id_game_name.as_str()]
            .into_iter()
            .filter(|n| !n.is_empty())
            .collect()
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

// --- Team advantage -------------------------------------------------------

/// Signed team differentials at one instant, from the active player's
/// point of view: positive means *our* team is ahead.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TeamDiff {
    /// "ORDER" or "CHAOS" — which side we were on. Persisted alongside the
    /// diffs so the sign convention stays auditable after the fact.
    pub our_team: String,
    /// **Estimated.** The Live Client Data API exposes no per-player gold
    /// at all — `activePlayer.currentGold` is our own *unspent* gold and is
    /// the only gold field in the entire response. This approximates each
    /// team's earned gold as the summed price of the items its players are
    /// currently holding, plus our unspent gold on our side only. It drifts
    /// from true gold via sold items, consumed consumables, component-vs-
    /// completed-item pricing, and the enemy's unknowable unspent gold, so
    /// it must never be presented to the user as an exact figure.
    pub gold_diff_est: f64,
    /// Exact, from `allPlayers[].scores`.
    pub kill_diff: i64,
    /// Exact, from `allPlayers[].scores`.
    pub cs_diff: i64,
}

/// Computes signed team differentials for one snapshot.
///
/// Returns `None` when we can't tell which side we're on — either
/// `activePlayer` is absent, or no `allPlayers` entry matches our name.
/// That's deliberately not a "default to ORDER" fallback: guessing wrong
/// silently inverts the sign of the whole curve, which would tell a user
/// they were ahead in every game they lost. Callers persist the `None` and
/// the UI renders "team side unknown" rather than an untrustworthy line.
///
/// This is the same name-matching failure mode that once silently dropped
/// every kill/death marker (see `matches_our_kills_when_events_use_summoner_name_not_riot_id`),
/// which is why it reuses `names_match` rather than comparing directly.
pub fn team_diff(snapshot: &AllGameData) -> Option<TeamDiff> {
    let active = snapshot.active_player.as_ref()?;
    let our_names = active.candidate_names();

    let our_team = snapshot
        .all_players
        .iter()
        .find(|p| {
            p.candidate_names()
                .iter()
                .any(|n| our_names.iter().any(|us| names_match(n, us)))
        })
        .map(|p| p.team.clone())
        .filter(|t| !t.is_empty())?;

    let (mut our_gold, mut their_gold) = (active.current_gold, 0.0);
    let (mut our_kills, mut their_kills) = (0i64, 0i64);
    let (mut our_cs, mut their_cs) = (0i64, 0i64);

    for player in &snapshot.all_players {
        let ours = player.team == our_team;
        let (gold, kills, cs) = if ours {
            (&mut our_gold, &mut our_kills, &mut our_cs)
        } else {
            (&mut their_gold, &mut their_kills, &mut their_cs)
        };
        *gold += player.item_gold();
        *kills += player.scores.kills;
        *cs += player.scores.creep_score;
    }

    Some(TeamDiff {
        our_team,
        gold_diff_est: our_gold - their_gold,
        kill_diff: our_kills - their_kills,
        cs_diff: our_cs - their_cs,
    })
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
fn classify_event(event: &GameEvent, our_names: &[&str]) -> Option<Marker> {
    let is_ours = |name: &Option<String>| -> bool {
        match name {
            Some(n) => our_names.iter().any(|us| names_match(n, us)),
            None => false,
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
            } else if event
                .assisters
                .iter()
                .any(|a| our_names.iter().any(|us| names_match(a, us)))
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

/// Compares names leniently: case-insensitive, and ignoring a `#tagline`
/// suffix if only one side has it. A live capture confirmed event
/// Killer/Victim/Assister fields are bare names with no tagline at all
/// (matching `summonerName`, not the `gameName#tagLine`-style `riotId`) —
/// the `#`-stripping is low-cost tolerance for a format that might still
/// show up in some other game mode, not a confirmed requirement.
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
        let our_names: Vec<&str> = snapshot
            .active_player
            .as_ref()
            .map(|p| p.candidate_names())
            .unwrap_or_default();

        let mut fresh = Vec::new();
        for event in &snapshot.events.events {
            if !self.seen_event_ids.insert(event.event_id) {
                continue;
            }
            if let Some(marker) = classify_event(event, &our_names) {
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

    /// Real capture (Practice Tool, 2026-09-01): `activePlayer` had both
    /// `summonerName` ("Ahri") and `riotIdGameName` ("NinjaGoldfinch")
    /// populated, but ChampionKill's Killer/Victim used `summonerName`.
    /// Regression coverage for the bug where preferring `riotIdGameName`
    /// unconditionally caused every kill/death marker for the player's
    /// own actions to silently vanish, with no error anywhere — objective
    /// markers (which don't need identity matching) worked fine, masking
    /// it until a live user noticed kills/deaths missing from a real game.
    fn summoner_name_mismatch_fixture() -> AllGameData {
        let json = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../fixtures/live-client/summoner-name-mismatch.json"
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
    fn matches_our_kills_when_events_use_summoner_name_not_riot_id() {
        let markers = MarkerTracker::new().ingest(&summoner_name_mismatch_fixture());
        let kinds: Vec<MarkerKind> = markers.iter().map(|m| m.kind).collect();

        assert!(kinds.contains(&MarkerKind::Kill), "kinds were: {kinds:?}");
        assert!(kinds.contains(&MarkerKind::Death), "kinds were: {kinds:?}");
        assert!(kinds.contains(&MarkerKind::FirstBlood), "kinds were: {kinds:?}");
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

    // --- team_diff -------------------------------------------------------
    //
    // Fixture is 2v2 rather than 5v5 on purpose: `team_diff` sums over
    // whichever players carry a given `team` string, so a 2v2 exercises the
    // identical code path as a full lobby while keeping the fixture (shared
    // with every marker test above) readable.
    //
    // Expected fixture values, ORDER = us:
    //   gold  ORDER items 6400 + 5300 = 11700, + our 450 unspent = 12150
    //         CHAOS items 4100 + 3200 =  7300           diff = +4850
    //   kills ORDER 3 + 2 = 5, CHAOS 2 + 1 = 3          diff =    +2
    //   cs    ORDER 150 + 120 = 270, CHAOS 130 + 95 = 225  diff =   +45

    #[test]
    fn team_diff_is_positive_when_our_team_is_ahead() {
        let diff = team_diff(&fixture()).expect("active player is in allPlayers");
        assert_eq!(diff.our_team, "ORDER");
        assert_eq!(diff.gold_diff_est, 4850.0);
        assert_eq!(diff.kill_diff, 2);
        assert_eq!(diff.cs_diff, 45);
    }

    /// The sign convention is the single most dangerous thing to get wrong
    /// here — an inverted curve would silently tell a user they were ahead
    /// in every game they lost. Re-pointing the active player at the losing
    /// side must flip every differential, not just relabel the team.
    #[test]
    fn team_diff_is_negative_when_we_are_on_the_losing_side() {
        let mut snapshot = fixture();
        // Become EnemyA (CHAOS) without touching anything else.
        snapshot.active_player = Some(ActivePlayer {
            summoner_name: String::new(),
            riot_id_game_name: "EnemyA".to_string(),
            current_gold: 450.0,
            level: 11,
        });

        let diff = team_diff(&snapshot).expect("EnemyA is in allPlayers");
        assert_eq!(diff.our_team, "CHAOS");
        // Same 450 unspent, now on the other side: 7750 - 11700.
        assert_eq!(diff.gold_diff_est, -3950.0);
        assert_eq!(diff.kill_diff, -2);
        assert_eq!(diff.cs_diff, -45);
    }

    #[test]
    fn team_diff_is_none_when_we_are_not_in_all_players() {
        let mut snapshot = fixture();
        snapshot.active_player = Some(ActivePlayer {
            riot_id_game_name: "SomeoneElse".to_string(),
            ..Default::default()
        });
        assert!(
            team_diff(&snapshot).is_none(),
            "must refuse to guess a side rather than risk inverting the curve"
        );
    }

    #[test]
    fn team_diff_is_none_without_an_active_player() {
        let mut snapshot = fixture();
        snapshot.active_player = None;
        assert!(team_diff(&snapshot).is_none());
    }

    /// A live response that omits `items` for enemies (unverified against a
    /// real game — see the plan's capture step) must still yield exact kill
    /// and CS diffs, with the gold estimate degrading rather than the whole
    /// snapshot failing.
    #[test]
    fn team_diff_survives_players_with_no_items() {
        let mut snapshot = fixture();
        for player in &mut snapshot.all_players {
            if player.team == "CHAOS" {
                player.items.clear();
            }
        }
        let diff = team_diff(&snapshot).unwrap();
        assert_eq!(diff.gold_diff_est, 12150.0, "our side only");
        assert_eq!(diff.kill_diff, 2, "scores are unaffected by missing items");
        assert_eq!(diff.cs_diff, 45);
    }

    /// `count` matters: Blitz holds 2 Control Wards at 350 each. A naive
    /// sum of `price` alone would under-count stacked consumables.
    #[test]
    fn item_gold_multiplies_by_stack_count() {
        let blitz = fixture()
            .all_players
            .into_iter()
            .find(|p| p.riot_id_game_name == "Blitz")
            .unwrap();
        assert_eq!(blitz.item_gold(), 5300.0); // 3300 + 1300 + 350*2
    }

    #[test]
    fn active_player_deserializes_gold_and_level() {
        let active = fixture().active_player.unwrap();
        assert_eq!(active.current_gold, 450.0);
        assert_eq!(active.level, 11);
    }

    /// The trimmed `summoner-name-mismatch.json` has no `allPlayers` at all.
    /// Deserialization must not fail (marker extraction still depends on it)
    /// and `team_diff` must simply report no side.
    #[test]
    fn snapshot_without_all_players_still_deserializes() {
        let snapshot = summoner_name_mismatch_fixture();
        assert!(snapshot.all_players.is_empty());
        assert!(team_diff(&snapshot).is_none());
    }

}
