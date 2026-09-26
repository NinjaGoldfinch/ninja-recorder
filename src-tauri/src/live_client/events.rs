//! Live Client Data types, marker extraction, and time alignment.
//! DEVELOPMENT.md §3.2, §3.4, §4.
//!
//! Marker classification (`classify_event`) and `MarkerTracker` are pure/
//! stateful-but-sync, so they're fully testable against fixture JSON
//! without a live poller — see the tests module and
//! `fixtures/live-client/sample-allgamedata.json`.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};
use crate::{debug, warn};
use std::collections::{BTreeSet, HashSet};
use std::sync::Mutex;

// --- Live Client Data response shape (subset we care about) ------------

#[derive(Debug, Clone, Deserialize)]
pub struct AllGameData {
    #[serde(rename = "activePlayer")]
    pub active_player: Option<ActivePlayer>,
    /// Every player in the game, both teams. Marker extraction doesn't need
    /// this (it matches our own name straight against event Killer/Victim/
    /// Assister fields — see `classify_event`), but the review timeline's
    /// advantage curve does: it's the only place the API exposes per-player
    /// scores, and the only way to learn which side we're on.
    #[serde(rename = "allPlayers", default, deserialize_with = "lenient_all_players")]
    pub all_players: Vec<PlayerEntry>,
    /// Defaulted like the lists: a snapshot with no event list is a snapshot
    /// with no new markers, not an unreadable one.
    #[serde(default)]
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
    /// Our own runes. Only the active player gets these in full — the
    /// other nine carry a reduced `runes` object — which is why the
    /// scoreboard records runes for us and not for them.
    #[serde(rename = "fullRunes", default)]
    pub full_runes: FullRunes,
}

/// The runes worth showing on a row: the keystone, and the two trees it
/// sits between. `generalRunes` and `statRunes` are in the response and
/// are left out — nine more icons on a row nobody is reading at that size.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct FullRunes {
    #[serde(default)]
    pub keystone: Rune,
    #[serde(rename = "primaryRuneTree", default)]
    pub primary_tree: Rune,
    #[serde(rename = "secondaryRuneTree", default)]
    pub secondary_tree: Rune,
}

/// A rune as the live API describes it. The `id` is what Data Dragon's
/// `runesReforged.json` files the icon under; the name is kept for the
/// case where the id resolves to nothing and the row needs words.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Rune {
    #[serde(default)]
    pub id: i64,
    #[serde(rename = "displayName", default)]
    pub display_name: String,
}

/// One entry from `allPlayers`. Every field is `default` because the
/// hand-trimmed fixtures omit most of them, and because a live response
/// that drops a field for enemies must degrade to a missing contribution
/// rather than failing the whole poll (and with it, marker extraction).
///
/// `items` came back for the scoreboard, and this time it carries
/// `itemID` — the thing Data Dragon files art under. It was modelled once
/// before with only `price` and `count`, for a gold estimate that turned
/// out to be unfixable (`lcu::timeline`), and removed with it.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PlayerEntry {
    #[serde(rename = "summonerName", default)]
    pub summoner_name: String,
    #[serde(rename = "riotIdGameName", default)]
    pub riot_id_game_name: String,
    /// The champion's display name, e.g. "Ahri". Taken straight from the
    /// response rather than resolved from an id, which is why the live
    /// path needs no id-to-name table at all (`lcu::match_data` does).
    #[serde(rename = "championName", default)]
    pub champion_name: String,
    /// The same champion as a string-table key,
    /// `game_character_displayname_Viego`. Read only to recognise Viego,
    /// because it does not depend on the client's language the way
    /// `championName` does (#203).
    #[serde(rename = "rawChampionName", default)]
    pub raw_champion_name: String,
    /// "ORDER" (blue side) or "CHAOS" (red side).
    #[serde(default)]
    pub team: String,
    #[serde(default)]
    pub level: i64,
    /// Where the game says this player is: `TOP`, `JUNGLE`, `MIDDLE`,
    /// `BOTTOM`, `UTILITY`, or empty in modes that have no lanes.
    ///
    /// This is the *game's* answer, not an inference. The LCU's
    /// `timeline.lane`/`role` pair is Riot working it out afterwards from
    /// where a player spent time, and it is weakest exactly between top
    /// and jungle — see `role` on `LiveSummary`.
    #[serde(default)]
    pub position: String,
    #[serde(default, deserialize_with = "lenient_player_items")]
    pub items: Vec<PlayerItem>,
    #[serde(rename = "summonerSpells", default)]
    pub summoner_spells: SummonerSpells,
    #[serde(default)]
    pub scores: PlayerScores,
}

/// One inventory slot. Only the id and the slot are read: the id is what
/// art is filed under, and the slot is what puts the trinket at the end
/// rather than wherever the response happened to list it.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PlayerItem {
    #[serde(rename = "itemID", default)]
    pub item_id: i64,
    #[serde(default)]
    pub slot: i64,
}

/// The two spells, as the response nests them.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SummonerSpells {
    #[serde(rename = "summonerSpellOne", default)]
    pub one: SummonerSpell,
    #[serde(rename = "summonerSpellTwo", default)]
    pub two: SummonerSpell,
}

/// A spell as the live API describes it. `displayName` is the human name
/// ("Flash"); Data Dragon files the art under a key ("SummonerFlash"), so
/// something has to map between them the way champion art already does.
/// The name is what gets stored, because it is the half that survives a
/// response shape changing.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SummonerSpell {
    #[serde(rename = "displayName", default)]
    pub display_name: String,
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
}

/// `kills` and `creep_score` feed the advantage curve and the scoreboard;
/// `deaths` and `assists` feed the library row's KDA (`self_summary`).
/// `wardScore` is in the real response too and is still left out — nothing
/// displays it yet, and modelling fields nothing reads is what the
/// original `allPlayers` comment was avoiding.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PlayerScores {
    #[serde(default)]
    pub kills: i64,
    #[serde(default)]
    pub deaths: i64,
    #[serde(default)]
    pub assists: i64,
    #[serde(rename = "creepScore", default)]
    pub creep_score: i64,
}

impl ActivePlayer {
    /// Every name this player might be referred to by in event
    /// Killer/Victim/Assister fields.
    ///
    /// **The event fields use `summonerName`, and `summonerName` is the
    /// champion name.** A Practice Tool capture (2026-09-01) suggested
    /// this and was written off as coincidence — that summoner name
    /// happened to be "Ahri". A real Ranked Solo capture (2026-09-07)
    /// settles it: `summonerName` was "Shyvana" while `riotIdGameName`
    /// was "NinjaGoldfinch" and `riotId` was "NinjaGoldfinch#OCENZ", and
    /// every event named "Shyvana".
    ///
    /// The same capture shows the API **anonymises everyone but us**: all
    /// nine other players came back with `riotId` "#" and an empty
    /// `riotIdGameName`, leaving the champion name as the only thing they
    /// can be identified by. So the champion name is not merely what these
    /// fields happen to use — for nine of the ten players it is the only
    /// name the payload contains at all.
    ///
    /// Matching against every non-empty candidate rather than picking one
    /// stays the right shape regardless: it costs nothing, and it means a
    /// mode that does populate Riot IDs cannot break marker attribution.
    pub fn candidate_names(&self) -> Vec<&str> {
        [self.summoner_name.as_str(), self.riot_id_game_name.as_str()]
            .into_iter()
            .filter(|n| !n.is_empty())
            .collect()
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct EventsWrapper {
    #[serde(rename = "Events", default, deserialize_with = "lenient_events")]
    pub events: Vec<GameEvent>,
}

/// Deserializes the event list **entry by entry**, dropping any it cannot
/// read instead of failing the whole snapshot.
///
/// This is the difference between losing one marker and losing a
/// recording. The events array grows during a game, so it is where a shape
/// nobody here has seen is most likely to arrive mid-game and take the
/// payload with it. In #74 something did, nine minutes in, and the recording
/// ended.
///
/// Riot may also add event types we have never modelled. Being unable to
/// read one of those should cost that event and nothing else.
///
/// Since #305 every other list in a snapshot goes through the same reader,
/// `lenient_list`, because defaulting a field only covers it being *missing*.
fn lenient_events<'de, D>(deserializer: D) -> Result<Vec<GameEvent>, D::Error>
where
    D: Deserializer<'de>,
{
    lenient_list(deserializer, "events.Events")
}

/// Reads a list **without letting its shape fail the snapshot** (#305).
///
/// One `Vec` field in `allgamedata` arriving as a JSON object once made every
/// poll for the rest of a game unreadable: no markers, no samples, no
/// scoreboard, and the trim then took the silence for post-game. Whatever the
/// cause, one field must cost at most that field. So a list is accepted as:
///
/// - an **array**, read entry by entry, skipping any entry that does not
///   parse, exactly as `lenient_events` always has;
/// - an **object**, taken as its values. Keys that are all whole numbers
///   (`{"0": …, "1": …}`, the shape a sparse array serialises to) put the
///   values in numeric order; anything else keeps the map's own order;
/// - **null, missing, or a scalar**: empty.
///
/// The first time a field is coerced a warning names it, once per process:
/// that line is how the next run on a real box says *which* field it was.
fn lenient_list<'de, D, T>(deserializer: D, field: &'static str) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    let values = match Option::<serde_json::Value>::deserialize(deserializer)? {
        None | Some(serde_json::Value::Null) => Vec::new(),
        Some(serde_json::Value::Array(items)) => items,
        Some(serde_json::Value::Object(map)) => {
            note_coerced(field, "an object");
            object_values(map)
        }
        Some(other) => {
            note_coerced(field, json_kind(&other));
            Vec::new()
        }
    };
    let mut out = Vec::with_capacity(values.len());
    for value in values {
        // Borrowing deserializer, so the value survives for the log line
        // on the failure path without cloning on the success path.
        match T::deserialize(&value) {
            Ok(item) => out.push(item),
            // Debug, not warn: every list is re-sent on every poll, so one
            // bad entry repeats each second for the rest of the game. Fixture
            // capture (DEVELOPMENT.md §3.3) is what preserves the shape
            // itself.
            Err(e) => debug!("live-client", "skipped an unreadable entry in {field} ({e}): {value}"),
        }
    }
    Ok(out)
}

/// An object's values as a list: in numeric key order when every key is a
/// whole number, and in the map's own order otherwise.
fn object_values(map: serde_json::Map<String, serde_json::Value>) -> Vec<serde_json::Value> {
    let numbered: Option<Vec<u64>> = map.keys().map(|key| key.trim().parse().ok()).collect();
    match numbered {
        Some(keys) => {
            let mut pairs: Vec<(u64, serde_json::Value)> =
                keys.into_iter().zip(map.into_iter().map(|(_, value)| value)).collect();
            pairs.sort_by_key(|(n, _)| *n);
            pairs.into_iter().map(|(_, value)| value).collect()
        }
        None => map.into_iter().map(|(_, value)| value).collect(),
    }
}

fn json_kind(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "a bool",
        serde_json::Value::Number(_) => "a number",
        serde_json::Value::String(_) => "a string",
        serde_json::Value::Array(_) => "an array",
        serde_json::Value::Object(_) => "an object",
    }
}

/// Says, once per field per process, that a list arrived as something else.
///
/// Once, because the payload is re-sent every second and the same coercion
/// would otherwise be most of the log. Warn, because it is the one line that
/// names a shape nobody here has seen, and that is worth reading.
fn note_coerced(field: &'static str, kind: &'static str) {
    static SEEN: Mutex<BTreeSet<&'static str>> = Mutex::new(BTreeSet::new());
    let first = match SEEN.lock() {
        Ok(mut seen) => seen.insert(field),
        Err(poisoned) => poisoned.into_inner().insert(field),
    };
    if first {
        warn!(
            "live-client",
            "{field} arrived as {kind} rather than a list and was read leniently \
             (said once per field)"
        );
    }
}

/// One `deserialize_with` function per lenient list: serde can only name a
/// function, and the field's name is what the log line needs.
macro_rules! lenient_lists {
    ($($name:ident => $field:literal,)*) => {$(
        fn $name<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
        where
            D: Deserializer<'de>,
            T: DeserializeOwned,
        {
            lenient_list(deserializer, $field)
        }
    )*};
}

lenient_lists! {
    lenient_all_players => "allPlayers",
    lenient_player_items => "allPlayers[].items",
    lenient_assisters => "events.Events[].Assisters",
    lenient_scoreboard_players => "scoreboard.players",
    lenient_scoreboard_items => "scoreboard.players[].items",
    lenient_scoreboard_spells => "scoreboard.players[].spells",
    lenient_scoreboard_spell_ids => "scoreboard.players[].spell_ids",
}

/// `Stolen` and friends: accept the value however this client spells it.
///
/// Every one of these shapes was written from documentation rather than
/// from a captured response, and a field present with an unexpected *type*
/// fails deserialization even when it is `Option` and `default` — `default`
/// only covers a missing key. Riot has historically sent booleans in this
/// API as the strings `"True"`/`"False"`, so a bare `Option<bool>` is a
/// live grenade on an event type we cannot test against.
///
/// An unrecognised value reads as `None` rather than erroring: not knowing
/// whether a baron was stolen is worth strictly less than the recording.
fn flexible_bool<'de, D>(deserializer: D) -> Result<Option<bool>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(match Option::<serde_json::Value>::deserialize(deserializer)? {
        Some(serde_json::Value::Bool(b)) => Some(b),
        Some(serde_json::Value::String(s)) => match s.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" => Some(true),
            "false" | "0" | "no" => Some(false),
            _ => None,
        },
        Some(serde_json::Value::Number(n)) => n.as_i64().map(|i| i != 0),
        _ => None,
    })
}

/// The same tolerance for a whole number — `KillStreak` on a `Multikill`.
/// See `flexible_bool`.
fn flexible_i64<'de, D>(deserializer: D) -> Result<Option<i64>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(match Option::<serde_json::Value>::deserialize(deserializer)? {
        Some(serde_json::Value::Number(n)) => n.as_i64(),
        Some(serde_json::Value::String(s)) => s.trim().parse().ok(),
        _ => None,
    })
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
    #[serde(rename = "Assisters", default, deserialize_with = "lenient_assisters")]
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
    #[serde(rename = "InhibKilled", default)]
    pub inhib_killed: Option<String>,
    /// On `Multikill`: 2 for a double, 5 for a penta.
    #[serde(rename = "KillStreak", default, deserialize_with = "flexible_i64")]
    pub kill_streak: Option<i64>,
    /// On the neutral-objective kills. A stolen Baron is precisely the
    /// moment someone scrubs back to find, so it rides in the payload.
    #[serde(rename = "Stolen", default, deserialize_with = "flexible_bool")]
    pub stolen: Option<bool>,
    /// Only ever set on the `GameEnd` event: "Win" or "Lose", from the
    /// active player's point of view. This is the whole of the live path's
    /// win/loss detection — see `outcome`.
    #[serde(rename = "Result", default)]
    pub result: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GameData {
    #[serde(rename = "gameTime")]
    pub game_time: f64,
    /// "CLASSIC", "ARAM", "PRACTICETOOL", … Not a queue id: the Live
    /// Client Data API never exposes one, so this is the closest the live
    /// path gets to naming the mode, and it lands in `recordings.game_mode`
    /// rather than `recordings.queue` (which is an INTEGER holding Riot's
    /// real queue id, and only the LCU can fill it).
    #[serde(rename = "gameMode", default)]
    pub game_mode: String,
}

// --- Team advantage -------------------------------------------------------

/// Signed team differentials at one instant, from the active player's
/// point of view: positive means *our* team is ahead.
// `Deserialize` too: WS3.4 reads these back off the pipe in the UI process.
#[derive(Debug, Clone, PartialEq, Serialize, ts_rs::TS, serde::Deserialize)]
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
/// which is why the lookup lives in `find_us` — shared with `self_summary`
/// and built on `names_match` — rather than comparing names directly here.
pub fn team_diff(snapshot: &AllGameData) -> Option<TeamDiff> {
    let our_team = find_us(snapshot)
        .map(|p| p.team.clone())
        .filter(|t| !t.is_empty())?;

    let (mut our_kills, mut their_kills) = (0i64, 0i64);
    let (mut our_cs, mut their_cs) = (0i64, 0i64);

    for player in &snapshot.all_players {
        let ours = player.team == our_team;
        let (kills, cs) = if ours {
            (&mut our_kills, &mut our_cs)
        } else {
            (&mut their_kills, &mut their_cs)
        };
        *kills += player.scores.kills;
        *cs += player.scores.creep_score;
    }

    Some(TeamDiff {
        our_team,
        kill_diff: our_kills - their_kills,
        cs_diff: our_cs - their_cs,
    })
}

/// The `allPlayers` entry for the player doing the recording, if we can
/// identify one. Shared by `team_diff` and `self_summary` so there is
/// exactly one answer in this module to "which of these ten players are
/// we", and one place to fix when it turns out to be wrong.
///
/// `None` when `activePlayer` is absent or no entry matches. See
/// `team_diff` for why that stays `None` instead of falling back.
fn find_us(snapshot: &AllGameData) -> Option<&PlayerEntry> {
    let our_names = snapshot.active_player.as_ref()?.candidate_names();
    snapshot.all_players.iter().find(|p| {
        p.candidate_names()
            .iter()
            .any(|n| our_names.iter().any(|us| names_match(n, us)))
    })
}

/// Our own kill participation, exact, from `allPlayers[].scores`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct Kda {
    pub kills: i64,
    pub deaths: i64,
    pub assists: i64,
}

/// What one Live Client Data snapshot says about the recording player's
/// own game — everything the library card shows that doesn't need the
/// LCU. Written to the `recordings` row as the polls establish it, and
/// again at finalize (DEVELOPMENT.md §4.3): the second write is what a
/// finished recording gets, the first is what a killed daemon leaves
/// behind.
///
/// Every field is optional because every field maps to a nullable column,
/// and because each has its own way of being unknowable: `champion` and
/// `kda` need us to be findable in `allPlayers`, `game_mode` doesn't, and
/// `win` isn't knowable at all until the game actually ends.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct LiveSummary {
    pub champion: Option<String>,
    pub kda: Option<Kda>,
    pub game_mode: Option<String>,
    /// `None` means "not decided yet", never "lost" — see `outcome`.
    pub win: Option<bool>,
    /// Where we played, from `allPlayers[].position`.
    ///
    /// **The game's answer, not Riot's inference.** The LCU's
    /// `timeline.lane`/`role` pair is derived after the fact from where a
    /// player spent time, and it confuses top with jungle often enough to
    /// put the wrong word on a row. This is what the client itself
    /// reported while the game was running, so it wins — and
    /// `update_match_metadata` fills `role` only when it is NULL for
    /// exactly that reason.
    pub role: Option<String>,
    /// Whether `champion` was reported as Viego, which is the one champion
    /// the API can report as somebody else (#203). Not a column and not on
    /// the wire: it exists so `absorb` can tell a possession from a pick.
    #[serde(skip)]
    pub viego: bool,
}

impl LiveSummary {
    /// Folds a newer snapshot into this one. The newer values win, except
    /// that a field we already know is never given back for a `None`.
    ///
    /// That asymmetry is the whole point. `GameEnd` shows up in the event
    /// list on one poll, and the game process can exit before the next one
    /// lands — so an outcome, once seen, has to survive however many empty
    /// polls follow it. The same holds for a snapshot that briefly fails
    /// to match us in `allPlayers`.
    ///
    /// `champion` has one more exception, decided by `settle_champion`: a
    /// Viego stays a Viego.
    pub fn absorb(&mut self, newer: LiveSummary) {
        (self.champion, self.viego) = settle_champion(
            (self.champion.take(), self.viego),
            (newer.champion, newer.viego),
        );
        if newer.kda.is_some() {
            self.kda = newer.kda;
        }
        if newer.game_mode.is_some() {
            self.game_mode = newer.game_mode;
        }
        if newer.win.is_some() {
            self.win = newer.win;
        }
        if newer.role.is_some() {
            self.role = newer.role;
        }
    }
}

/// Which champion a recording was played on, given what it knew and what the
/// newest poll says. Each side is `(champion, reported as Viego)`.
///
/// **A possession is not a pick** (#203). Viego's passive takes over a
/// champion he helped kill, and while it lasts Live Client Data reports him
/// under that champion's name. A game that ended mid-possession therefore
/// left its last poll saying `Malphite` about a Viego, and "the newer value
/// wins" wrote that to the row. No other champion changes mid-game (a Gnar's
/// forms are folded back by `normalize_champion`), so:
///
/// - A poll that reports Viego is believed, whatever came before. It is also
///   what corrects a recording that started mid-possession, once the
///   possession ends.
/// - Once Viego has been seen, any other name is a possession and is ignored.
/// - Otherwise the newer value wins, as it does for every other field.
///
/// A recording that sees only a possession, never Viego himself, keeps the
/// possessed name: nothing in the live data can tell it apart. That is what
/// the deferred patch's `correct_champion` is for, where the LCU answers.
pub(crate) fn settle_champion(
    known: (Option<String>, bool),
    newer: (Option<String>, bool),
) -> (Option<String>, bool) {
    match (known, newer) {
        (_, (Some(name), true)) => (Some(name), true),
        (known @ (Some(_), true), _) => known,
        (_, (Some(name), false)) => (Some(name), false),
        (known, (None, _)) => known,
    }
}

/// Whether this entry is Viego, by the language-independent key when the
/// response carries one and by the display name when it does not (the
/// hand-trimmed fixtures, and any client that drops the field).
///
/// **Unverified off Windows: what `rawChampionName` says mid-possession.**
/// `championName` is known to change (#203). If the key changes with it, a
/// possessing poll is not Viego here and `settle_champion` ignores it; if the
/// key stays `Viego`, it *is* Viego here and `self_summary` names it `Viego`
/// rather than taking the display name. Either way the row says Viego, which
/// is why the answer does not have to be known.
fn is_viego(player: &PlayerEntry) -> bool {
    player.raw_champion_name.trim() == "game_character_displayname_Viego"
        || player.champion_name.trim() == "Viego"
}

/// Live Client Data's position, in the words the `role` column already
/// holds.
///
/// The vocabulary has to match `lcu::match_data::position` exactly: one
/// column, two writers, and a column carrying both `Support` and `UTILITY`
/// is one nothing can group by. Same rule as champion names.
///
/// An unrecognised value — including the empty string a mode with no lanes
/// reports — yields `None`, which the row renders as `Unknown`. A guess
/// here would be a guess in a slot people read as fact.
fn live_position(position: &str) -> Option<String> {
    let named = match position.trim().to_ascii_uppercase().as_str() {
        "TOP" => "Top",
        "JUNGLE" => "Jungle",
        "MIDDLE" | "MID" => "Middle",
        "BOTTOM" | "BOT" => "Bottom",
        "UTILITY" | "SUPPORT" => "Support",
        _ => return None,
    };
    Some(named.to_string())
}

/// The end-of-game scoreboard, as the library row wants to draw it.
///
/// Serialized whole into `recordings.scoreboard_json` rather than spread
/// across columns: nothing filters or sorts on the other nine players, it
/// is always read in one piece, and a column is disposed of with its row
/// so retention needs no cascade. Same reasoning as `audio_tracks_json`
/// and `diagnostics_json` (docs/data-model.md).
///
/// Our own CS is the exception and gets a real column, because it is shown
/// on the row and is worth sorting by.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
pub struct Scoreboard {
    /// Both teams, in the order the response listed them.
    #[serde(default, deserialize_with = "lenient_scoreboard_players")]
    pub players: Vec<ScoreboardPlayer>,
    /// `"ORDER"` or `"CHAOS"`, or absent when we could not be matched —
    /// in which case the row cannot say which half is ours and shows
    /// neither.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub our_team: Option<String>,
    /// Ours only: the live API gives the full rune page for the active
    /// player and a reduced one for everybody else.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub our_runes: Option<ScoreboardRunes>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
pub struct ScoreboardPlayer {
    pub champion: String,
    /// `"ORDER"` or `"CHAOS"`.
    pub team: String,
    /// True for the row's owner, so the frontend does not have to match
    /// names a second time — that question has one answer here (`find_us`)
    /// and it should not grow a second one in TypeScript.
    #[serde(default, skip_serializing_if = "is_false")]
    pub is_us: bool,
    pub level: i64,
    pub kills: i64,
    pub deaths: i64,
    pub assists: i64,
    pub cs: i64,
    /// `Top` / `Jungle` / `Middle` / `Bottom` / `Support`, in the same words
    /// the `role` column uses.
    ///
    /// Carried for every player, not just ours, because it is the only thing
    /// that can say which of the five opponents was *the* opponent — the row
    /// draws a matchup, and a lane opponent picked by list order would be a
    /// guess dressed as a fact. `None` where the game said nothing, which is
    /// every recording made before this field existed and any mode with no
    /// positions to assign.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<String>,
    /// Item ids in slot order, trinket included. Empty slots are dropped
    /// rather than zero-filled: a zero is an item id that does not exist,
    /// and the row draws as many boxes as it wants regardless.
    #[serde(default, deserialize_with = "lenient_scoreboard_items")]
    pub items: Vec<i64>,
    /// Spell display names — `["Flash", "Smite"]`. Names rather than the
    /// keys Data Dragon files art under, because the name is the half that
    /// survives a response shape changing; mapping one to the other is the
    /// art layer's job, as it already is for champions.
    #[serde(default, deserialize_with = "lenient_scoreboard_spells")]
    pub spells: Vec<String>,
    /// The same two spells as ids, which is all match history gives.
    ///
    /// A scoreboard captured live has names and no ids; one rebuilt from
    /// match history has ids and no names. Both are enough to find the
    /// art, so both are stored rather than one being converted into the
    /// other — converting would need the CDN, in a path that otherwise
    /// only talks to the League client.
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "lenient_scoreboard_spell_ids"
    )]
    pub spell_ids: Vec<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
pub struct ScoreboardRunes {
    pub keystone_id: i64,
    pub keystone: String,
    pub primary_tree_id: i64,
    pub secondary_tree_id: i64,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// Reduces one snapshot to the scoreboard the row draws.
///
/// `None` when the response carried no players at all, which is what a
/// poll during the loading screen looks like — an empty scoreboard is not
/// the end-of-game state, it is the absence of one, and storing it would
/// overwrite a real one captured earlier.
pub fn scoreboard(snapshot: &AllGameData) -> Option<Scoreboard> {
    if snapshot.all_players.is_empty() {
        return None;
    }

    let us = find_us(snapshot);
    let our_names: Vec<&str> = us.map(|p| p.candidate_names()).unwrap_or_default();
    let is_us = |player: &PlayerEntry| {
        !our_names.is_empty()
            && player
                .candidate_names()
                .iter()
                .any(|n| our_names.iter().any(|ours| names_match(n, ours)))
    };

    Some(Scoreboard {
        players: snapshot
            .all_players
            .iter()
            .map(|player| {
                let mut items: Vec<(i64, i64)> = player
                    .items
                    .iter()
                    .filter(|item| item.item_id > 0)
                    .map(|item| (item.slot, item.item_id))
                    .collect();
                // Slot order, because the response's order is not promised
                // to be it and a build that reshuffles between polls would
                // make the row flicker.
                items.sort_by_key(|(slot, _)| *slot);

                ScoreboardPlayer {
                    champion: normalize_champion(&player.champion_name),
                    team: player.team.clone(),
                    is_us: is_us(player),
                    level: player.level,
                    kills: player.scores.kills,
                    deaths: player.scores.deaths,
                    assists: player.scores.assists,
                    cs: player.scores.creep_score,
                    position: live_position(&player.position),
                    items: items.into_iter().map(|(_, id)| id).collect(),
                    spells: [
                        player.summoner_spells.one.display_name.clone(),
                        player.summoner_spells.two.display_name.clone(),
                    ]
                    .into_iter()
                    .filter(|name| !name.is_empty())
                    .collect(),
                    // The live API gives names; ids are the match-history
                    // rebuild's half of this.
                    spell_ids: Vec::new(),
                }
            })
            .collect(),
        our_team: us.map(|p| p.team.clone()).filter(|t| !t.is_empty()),
        our_runes: snapshot.active_player.as_ref().and_then(|active| {
            let runes = &active.full_runes;
            // A rune page with no keystone id is the loading screen's
            // empty shape, not a page worth storing.
            (runes.keystone.id > 0).then(|| ScoreboardRunes {
                keystone_id: runes.keystone.id,
                keystone: runes.keystone.display_name.clone(),
                primary_tree_id: runes.primary_tree.id,
                secondary_tree_id: runes.secondary_tree.id,
            })
        }),
    })
}

/// Extracts the recording player's own champion, KDA, game mode and (once
/// the game has ended) result from one snapshot.
///
/// Deliberately infallible: `game_mode` is readable even when we can't
/// work out which player we are, and dropping it just because the name
/// match failed would lose the one label a Practice Tool recording can
/// otherwise show.
/// One line describing what a poll actually showed, for the diagnostic log
/// (DEVELOPMENT.md §13).
///
/// **Distilled, not raw.** A real `allgamedata` response is tens of
/// kilobytes and arrives at 1 Hz, so keeping every payload would cost
/// hundreds of megabytes for a single game. This is about 120 bytes, or
/// roughly 200 KB across a game — enough to answer "what was the app
/// seeing when this went wrong" without that bill. When the raw stream is
/// genuinely needed, fixture capture (DEVELOPMENT.md §3.3) is where it
/// lives.
///
/// Fixed key order and fixed key set, including when a value is unknown:
/// a line whose shape changes with its content is one nothing can grep or
/// parse. Unknown reads as `-`.
///
/// `matched` is the one that earns its place twice over — the Practice
/// Tool name ambiguity documented on `find_us` means "we could not tell
/// which player is us" is a real, recurring state, and it silently empties
/// champion, KDA and the advantage curve.
pub fn poll_trace(
    snapshot: &AllGameData,
    elapsed_s: f64,
    alignment: Option<TimeAlignment>,
    new_markers: usize,
) -> String {
    let us = find_us(snapshot);
    let offset = match alignment {
        Some(a) => format!("{:.2}", a.offset_s()),
        // The clock has not been seen to advance yet — a loading screen, a
        // pause, or a game that ended before it ever ticked.
        None => "-".to_string(),
    };
    let (champion, kda) = match us {
        Some(p) => (
            if p.champion_name.trim().is_empty() {
                "-".to_string()
            } else {
                normalize_champion(&p.champion_name)
            },
            format!("{}/{}/{}", p.scores.kills, p.scores.deaths, p.scores.assists),
        ),
        None => ("-".to_string(), "-".to_string()),
    };
    // Gold and level come off `activePlayer`, not the `allPlayers` entry —
    // the API only exposes them for us, and it can answer for them even on
    // a poll where the name match failed.
    let (gold, level) = match snapshot.active_player.as_ref() {
        Some(a) => (format!("{:.0}", a.current_gold), a.level.to_string()),
        None => ("-".to_string(), "-".to_string()),
    };

    format!(
        "game={:.1} cap={:.1} off={} matched={} champ={} kda={} gold={} lvl={} events={} new={}",
        snapshot.game_data.game_time,
        elapsed_s,
        offset,
        if us.is_some() { "yes" } else { "no" },
        champion,
        kda,
        gold,
        level,
        snapshot.events.events.len(),
        new_markers,
    )
}

/// Turns what Live Client Data calls a champion into what Data Dragon files
/// it under.
///
/// **The API reports the champion's current *form*, not its identity.** A
/// transformed Gnar comes back as `Mega Gnar`, which is not a champion any
/// asset store has heard of: the portrait lookup finds nothing, and the
/// scoreboard slot renders as an empty black square for the rest of the
/// game — which is also what "no art yet" looks like, so it does not even
/// read as a fault.
///
/// Deliberately a rename table rather than a "strip a known prefix" rule.
/// The set of transforming champions is small, fixed and known, and a rule
/// that guessed would eventually eat a real champion whose name starts with
/// a word that looks like a form.
///
/// The general name → key mapping is *not* done here — `ddragon` already
/// resolves `Wukong` to `MonkeyKing` from the client's own asset store,
/// which is the right place for it because it stays correct when Riot adds
/// a champion. This is only for names that are not champions at all.
///
/// `ddragon::champion_icon` calls this too, and needs to: normalizing here
/// only fixes recordings made *after* it existed. Rows already carrying
/// `Mega Gnar` are on disk, and the art path is where they get a portrait
/// back without rewriting the database.
pub(crate) fn normalize_champion(name: &str) -> String {
    let name = name.trim();
    match name {
        "Mega Gnar" => "Gnar".to_string(),
        other => other.to_string(),
    }
}

pub fn self_summary(snapshot: &AllGameData) -> LiveSummary {
    let us = find_us(snapshot);
    let viego = us.is_some_and(is_viego);

    LiveSummary {
        // `Viego` by name whenever the entry is Viego by key, so a possession
        // the key sees through cannot put the possessed name on the row. The
        // English name, as `normalize_champion` and `ddragon`'s `en_US`
        // lookups already assume.
        champion: if viego {
            Some("Viego".to_string())
        } else {
            us.map(|p| p.champion_name.trim())
                .filter(|c| !c.is_empty())
                .map(normalize_champion)
        },
        viego,
        kda: us.map(|p| Kda {
            kills: p.scores.kills,
            deaths: p.scores.deaths,
            assists: p.scores.assists,
        }),
        game_mode: Some(snapshot.game_data.game_mode.trim())
            .filter(|m| !m.is_empty())
            .map(str::to_string),
        win: outcome(snapshot),
        role: us.and_then(|p| live_position(&p.position)),
    }
}

/// Win/loss from the `GameEnd` event's `Result` field, which is the only
/// place the Live Client Data API states an outcome.
///
/// An unrecognised value yields `None` rather than `false`. A recording
/// wrongly badged as a loss is worse than one badged as unknown: the card
/// already renders unknown honestly, and the win-rate tile deliberately
/// excludes it (`renderStats` in `src/library.ts`).
fn outcome(snapshot: &AllGameData) -> Option<bool> {
    let result = snapshot
        .events
        .events
        .iter()
        .find(|e| e.event_name == "GameEnd")
        .and_then(|e| e.result.as_deref())?;

    match result.trim().to_ascii_lowercase().as_str() {
        "win" => Some(true),
        "lose" => Some(false),
        _ => None,
    }
}

// --- Markers -------------------------------------------------------------

// `Deserialize` too: WS3.4 reads these back off the pipe in the UI process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ts_rs::TS, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MarkerKind {
    Kill,
    Death,
    Assist,
    Dragon,
    Baron,
    Herald,
    /// The Voidgrubs. The API calls the event `HordeKill` and the objective
    /// the Horde; players call them grubs, and the timeline is read by
    /// players.
    Voidgrubs,
    Turret,
    Inhibitor,
    Ace,
    Multikill,
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
            MarkerKind::Voidgrubs => "voidgrubs",
            MarkerKind::Turret => "turret",
            MarkerKind::Inhibitor => "inhibitor",
            MarkerKind::Ace => "ace",
            MarkerKind::Multikill => "multikill",
            MarkerKind::FirstBlood => "first_blood",
        }
    }
}

// `Deserialize` too: WS3.4 reads these back off the pipe in the UI process.
#[derive(Debug, Clone, PartialEq, Serialize, ts_rs::TS, serde::Deserialize)]
pub struct Marker {
    pub kind: MarkerKind,
    pub game_time_s: f64,
    /// Structured detail specific to the marker kind (killer/victim/dragon
    /// type/etc.) — matches the `payload_json` column planned in
    /// DEVELOPMENT.md §4, so this serializes straight into the DB.
    ///
    /// `unknown` rather than `any` on the TypeScript side, and deliberately:
    /// this really is an untyped blob today, and `unknown` forces a consumer
    /// to narrow it before use where `any` would let a typo through silently.
    ///
    /// The shape it *should* have is a discriminated union keyed on `kind` —
    /// `Kill { killer, victim }`, `Dragon { kind }`, and so on. That is a real
    /// change to this type rather than an annotation, so it is not WS2.2's.
    #[ts(type = "unknown")]
    pub payload: serde_json::Value,
}

/// Classifies one event into a marker, or `None` if it's not a kind we
/// track (`GameStart`, `MinionsSpawning`, etc.) or not one we took part
/// in.
///
/// **Every kind is gated on us being named in the event**, not just
/// `ChampionKill`. A marker is a seek target and a stop on the review
/// player's `[`/`]` navigation, so the bar is "was this about me", not
/// "did this happen" — a turret a teammate took while we were on the far
/// side of the map is exactly the stop nobody wants. The gate reads the
/// event's own killer/victim/assister/acer/recipient fields and never
/// team membership, and drops here so an uninvolved event never reaches
/// the database. That is irreversible per recording: Live Client Data is
/// gone once the game ends. The trade, and the two alternatives rejected
/// for it, are in DEVELOPMENT.md §3.2.
fn classify_event(event: &GameEvent, our_names: &[&str]) -> Option<Marker> {
    let is_ours = |name: &Option<String>| -> bool {
        match name {
            Some(n) => our_names.iter().any(|us| names_match(n, us)),
            None => false,
        }
    };
    let assisted = || {
        event
            .assisters
            .iter()
            .any(|a| our_names.iter().any(|us| names_match(a, us)))
    };
    // Did we have a hand in this at all? Every objective below is gated on
    // it, so a turret our team took while we were on the other side of the
    // map never becomes a seek target.
    let took_part = || is_ours(&event.killer_name) || assisted();

    let marker = |kind: MarkerKind, payload: serde_json::Value| {
        Some(Marker {
            kind,
            game_time_s: event.event_time,
            payload,
        })
    };

    match event.event_name.as_str() {
        "ChampionKill" => {
            if is_ours(&event.killer_name) {
                marker(
                    MarkerKind::Kill,
                    serde_json::json!({ "victim": event.victim_name }),
                )
            } else if is_ours(&event.victim_name) {
                marker(
                    MarkerKind::Death,
                    serde_json::json!({ "killer": event.killer_name }),
                )
            } else if assisted() {
                marker(
                    MarkerKind::Assist,
                    serde_json::json!({
                        "victim": event.victim_name,
                        "killer": event.killer_name,
                    }),
                )
            } else {
                None
            }
        }
        "Multikill" if is_ours(&event.killer_name) => marker(
            MarkerKind::Multikill,
            serde_json::json!({ "kill_streak": event.kill_streak }),
        ),
        "TurretKilled" if took_part() => marker(
            MarkerKind::Turret,
            serde_json::json!({
                "killer": event.killer_name,
                "turret": event.turret_killed,
            }),
        ),
        "InhibKilled" if took_part() => marker(
            MarkerKind::Inhibitor,
            serde_json::json!({
                "killer": event.killer_name,
                "inhibitor": event.inhib_killed,
            }),
        ),
        "DragonKill" if took_part() => marker(
            MarkerKind::Dragon,
            serde_json::json!({
                "killer": event.killer_name,
                "dragon_type": event.dragon_type,
                "stolen": event.stolen,
            }),
        ),
        "BaronKill" if took_part() => marker(
            MarkerKind::Baron,
            serde_json::json!({
                "killer": event.killer_name,
                "stolen": event.stolen,
            }),
        ),
        "HeraldKill" if took_part() => marker(
            MarkerKind::Herald,
            serde_json::json!({
                "killer": event.killer_name,
                "stolen": event.stolen,
            }),
        ),
        // The Voidgrubs, which the API files under `HordeKill` — one event
        // per grub, so a cleared camp is three markers a few seconds apart
        // rather than one. They are left that way: each is a separate kill
        // with its own `EventID`, the tracker dedupes on exactly that, and
        // collapsing them would mean inventing a window over which three
        // kills become one camp.
        "HordeKill" if took_part() => marker(
            MarkerKind::Voidgrubs,
            serde_json::json!({
                "killer": event.killer_name,
                "stolen": event.stolen,
            }),
        ),
        // `Acer` is whoever landed the final kill of the ace, so this is
        // "an ace I closed out", not "an ace my team got". Being *on* the
        // acing team without appearing anywhere in it is the same absent
        // bystander case as the turret above.
        "Ace" if is_ours(&event.acer) => marker(
            MarkerKind::Ace,
            serde_json::json!({
                "acer": event.acer,
                "acing_team": event.acing_team,
            }),
        ),
        // Deliberately not handled: `FirstBrick`, the first turret of the
        // game. The API also emits an ordinary `TurretKilled` for the same
        // structure with its own `EventID`, so the tracker would not dedupe
        // them and the VOD would carry two markers a frame apart.
        "FirstBlood" if is_ours(&event.recipient) => marker(
            MarkerKind::FirstBlood,
            serde_json::json!({ "recipient": event.recipient }),
        ),
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

/// How far before the moment capture began an event can be and still be
/// treated as this recording's. See `MarkerTracker::disown_earlier_events`.
///
/// Two seconds because both ways of getting the edge wrong are cheap at
/// that size, and only one of them is cheap at any size. Too small, and
/// jitter in the estimate of when capture began (the poll's own latency,
/// the gap between `Recorder::start` returning and `record_started_at`
/// being read) could disown an event that is really on the video: that
/// marker is gone for good, because Live Client Data does not outlive the
/// game. Too large, and an event from just before the start is kept: it
/// clamps to 0:00, which is within this many seconds of where it happened.
/// The second failure is the one to lean towards.
const INHERITED_EVENT_TOLERANCE_S: f64 = 2.0;

/// De-duplicates markers across repeated polls of the same game — each
/// poll returns the *entire* event list so far, not just what's new.
///
/// A tracker belongs to one recording, which is also what makes that list
/// a hazard at the start of one: the first poll hands over every event of
/// the game so far, and a recording that began mid-game (a daemon restarted
/// part-way through, #199; a reconnect) was not there for most of them.
/// `disown_earlier_events` is what keeps those off it.
#[derive(Debug, Default)]
pub struct MarkerTracker {
    seen_event_ids: HashSet<i64>,
    /// Whether `disown_earlier_events` has had its one call. Separate from
    /// `seen_event_ids` being empty, which it still is after a first poll
    /// whose event list was empty.
    disowned: bool,
}

impl MarkerTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Marks every event in the recording's **first** snapshot that happened
    /// before capture began as already seen, so `ingest` never turns it into
    /// a marker. Returns how many it disowned; every later call is a no-op
    /// and returns 0.
    ///
    /// `capture_began_at_game_time_s` is the game clock at the instant video
    /// time 0 was captured, estimated from the first poll as its `gameTime`
    /// less the capture time already elapsed when it landed.
    ///
    /// **Why.** The Live Client Data API serves the whole game's event list,
    /// not the events since the last poll, and dedupe is against this
    /// tracker's own memory. A recording that starts mid-game therefore used
    /// to take every earlier event as its own on its first poll, and since
    /// `TimeAlignment::video_time_s` clamps anything before the start to 0,
    /// they all piled up at 0:00 (#199). The recording that was running when
    /// they happened already has them, at the right positions.
    ///
    /// **Why game time, and not a negative video time.** The obvious filter
    /// is "drop anything that resolves to before the video started", and it
    /// is wrong on both paths a marker is written by. On the first poll of a
    /// mid-game recording no alignment has been proven yet, so a marker is
    /// written live against the 1:1 fallback, where game second 106 is video
    /// second 106 and nothing looks early (see `AlignmentTracker` and
    /// `the_finalize_corrects_a_sample_written_before_the_clock_moved`). And
    /// when the alignment itself is wrong, as it may be in a Practice Tool
    /// whose clock pauses or skips (#198), a video-time filter would drop the
    /// recording's *own* events along with the inherited ones, silently. This
    /// compares the event's game time with the game time at which capture
    /// began, both read off the same clock, so no alignment is involved and
    /// #198 cannot make it worse.
    ///
    /// **Why only the first snapshot.** Every event that could have been
    /// inherited is already in that list, because the list is cumulative.
    /// Anything that first appears later happened while this recording was
    /// running, whatever its timestamp says, so it is never disowned: a game
    /// clock that jumped backwards could not take one of the recording's own
    /// events with it. The same property is what makes a Live Client outage
    /// inside one recording harmless. The session and this tracker survive
    /// it, the polls after it are not a first snapshot, and dedupe on
    /// `EventID` carries on exactly as before.
    ///
    /// **The loading-screen case is unaffected.** A recording that starts
    /// before game time 0 sees a frozen `gameTime` of 0 on its first poll,
    /// with some capture already elapsed, so the estimate is at or below 0
    /// and no event is ever earlier than it. Errors in the estimate lean the
    /// same way: a clock frozen by a pause or a slow first poll makes it
    /// *earlier* than the truth, which keeps more rather than less.
    pub fn disown_earlier_events(
        &mut self,
        snapshot: &AllGameData,
        capture_began_at_game_time_s: f64,
    ) -> usize {
        if std::mem::replace(&mut self.disowned, true) {
            return 0;
        }
        let cutoff = capture_began_at_game_time_s - INHERITED_EVENT_TOLERANCE_S;
        let mut disowned = 0;
        for event in &snapshot.events.events {
            if event.event_time < cutoff && self.seen_event_ids.insert(event.event_id) {
                disowned += 1;
            }
        }
        disowned
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
///
/// One of these describes the mapping *at one instant*. It is not the whole
/// story for a recording: see `AlignmentTracker`, which produces a fresh one
/// per poll because the true offset moves during a game.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimeAlignment {
    offset_s: f64,
}

impl TimeAlignment {
    /// `game_time_s` and `elapsed_since_record_start_s` must be sampled at
    /// the *same* instant — the game clock the poll reported, and how long
    /// capture had been running when that poll landed. Any skew between the
    /// two lands directly in the offset.
    pub fn new(game_time_s: f64, elapsed_since_record_start_s: f64) -> Self {
        Self {
            offset_s: elapsed_since_record_start_s - game_time_s,
        }
    }

    /// Video-time position for a marker recorded at `game_time_s`. Clamped
    /// to 0 — a marker computed to land before the recording started (e.g.
    /// a backdated event right at game start) snaps to the beginning
    /// rather than producing a nonsensical negative seek target.
    pub fn video_time_s(&self, game_time_s: f64) -> f64 {
        (game_time_s + self.offset_s).max(0.0)
    }

    /// The raw offset. Negative means recording started *after* game time
    /// 0 (a reconnect); positive is the normal loading-screen case.
    ///
    /// This used to be `#[cfg(test)]`, because nothing in production read
    /// the offset on its own — callers map through `video_time_s`. The
    /// poll trace does read it: a wrong offset is the difference between a
    /// marker that seeks to the right moment and one that misses by
    /// twenty seconds, so it belongs in the log.
    pub fn offset_s(&self) -> f64 {
        self.offset_s
    }
}

/// Follows the game-time-to-video-time offset across a whole recording,
/// re-deriving it on every poll that proves the game clock is running.
///
/// Two problems rule out measuring the offset once, on the first poll:
///
/// 1. **The loading screen reports a frozen `gameTime` of 0.** Recording
///    starts on the first successful poll, so the first poll's `elapsed` is
///    ~0 too, and `0 - 0` says the video and the game start together. It
///    doesn't: the video contains the entire loading screen ahead of game
///    time 0, so every marker lands one load too early.
/// 2. **A single offset drifts.** A game pause freezes the clock while the
///    video keeps rolling, and dropped encoder frames skew the mapping over
///    a 40-minute game. An offset measured once at minute 0 is wrong by
///    minute 40.
///
/// So the tracker waits for `gameTime` to *advance* — proving it is a clock
/// and not the loading screen's frozen 0 — and from then on re-measures on
/// every advancing poll. Markers are stamped with the alignment in force
/// when they were observed, which is at most one poll (~1 s) old.
#[derive(Debug, Clone, Default)]
pub struct AlignmentTracker {
    last_game_time_s: Option<f64>,
    current: Option<TimeAlignment>,
    first: Option<TimeAlignment>,
}

impl AlignmentTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds one poll: the `gameTime` it reported and how long capture had
    /// been running when it landed. Returns the alignment to stamp on
    /// anything observed at this poll, or `None` while the clock is still
    /// frozen and no offset has ever been proven.
    pub fn observe(&mut self, game_time_s: f64, elapsed_s: f64) -> Option<TimeAlignment> {
        // Strictly greater: a run of identical timestamps is the loading
        // screen (or a pause), and re-deriving across it would fold the
        // frozen interval into the offset.
        if self.last_game_time_s.is_some_and(|last| game_time_s > last) {
            let alignment = TimeAlignment::new(game_time_s, elapsed_s);
            self.current = Some(alignment);
            self.first.get_or_insert(alignment);
        }
        // Tracked even while frozen, so the *next* poll can tell it moved.
        // A reconnect's first poll reports a live clock already at, say,
        // 600 — indistinguishable from a frozen one until it ticks, which
        // costs one poll of accuracy rather than the minutes a wrong offset
        // would cost.
        self.last_game_time_s = Some(game_time_s);
        self.current
    }

    /// The alignment for anything observed *before* the clock was ever seen
    /// to advance: the first one proven, or — if it never advanced, because
    /// the game ended during loading — a 1:1 mapping. Both beat dropping the
    /// markers.
    pub fn fallback(&self) -> TimeAlignment {
        self.first.unwrap_or(TimeAlignment { offset_s: 0.0 })
    }

    /// The offset currently in force: the dev portal's live readout, and
    /// the value the finalize records in `RecordingDiagnostics`.
    ///
    /// `None` means the clock was never seen to advance, which is a
    /// different thing from an offset of zero — markers then fall back to
    /// the first alignment the recording proved, or to 1:1.
    pub fn current_offset_s(&self) -> Option<f64> {
        self.current.map(|a| a.offset_s)
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

    /// A `GameEvent` with everything unset, for building one event by
    /// hand. Deliberately a test helper rather than a `Default` derive on
    /// `GameEvent` itself: a default *wire* event is not a thing the API
    /// can send, and `EventName: ""` would silently classify as nothing.
    fn blank_event() -> GameEvent {
        GameEvent {
            event_id: 0,
            event_name: String::new(),
            event_time: 0.0,
            killer_name: None,
            victim_name: None,
            assisters: Vec::new(),
            recipient: None,
            acer: None,
            acing_team: None,
            dragon_type: None,
            turret_killed: None,
            inhib_killed: None,
            kill_streak: None,
            stolen: None,
            result: None,
        }
    }

    fn won_fixture() -> AllGameData {
        let json = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../fixtures/live-client/game-end-win.json"
        ));
        serde_json::from_str(json).unwrap()
    }

    fn lost_fixture() -> AllGameData {
        let json = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../fixtures/live-client/game-end-lose.json"
        ));
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn self_summary_reads_our_champion_and_full_kda_from_all_players() {
        let summary = self_summary(&fixture());
        assert_eq!(summary.champion.as_deref(), Some("Ahri"));
        assert_eq!(
            summary.kda,
            Some(Kda {
                kills: 3,
                deaths: 1,
                assists: 2
            })
        );
        assert_eq!(summary.game_mode.as_deref(), Some("CLASSIC"));
    }

    #[test]
    fn a_game_still_in_progress_has_no_outcome_rather_than_a_loss() {
        // The sample fixture is a snapshot mid-game: no GameEnd event yet.
        // `false` here would badge every in-flight recording as a defeat.
        assert_eq!(self_summary(&fixture()).win, None);
    }

    #[test]
    fn game_end_result_decides_the_outcome() {
        assert_eq!(self_summary(&won_fixture()).win, Some(true));
        assert_eq!(self_summary(&lost_fixture()).win, Some(false));
    }

    #[test]
    fn an_unrecognized_result_is_unknown_not_a_loss() {
        let mut snapshot = won_fixture();
        for event in &mut snapshot.events.events {
            if event.event_name == "GameEnd" {
                event.result = Some("Surrendered".into());
            }
        }
        assert_eq!(outcome(&snapshot), None);
    }

    #[test]
    fn game_mode_survives_a_snapshot_we_cannot_place_ourselves_in() {
        // A Practice Tool recording where the name match fails still has a
        // mode worth showing on the card — losing it too would leave the
        // row with nothing at all.
        let mut snapshot = fixture();
        snapshot.all_players.clear();

        let summary = self_summary(&snapshot);
        assert_eq!(summary.champion, None);
        assert_eq!(summary.kda, None);
        assert_eq!(summary.game_mode.as_deref(), Some("CLASSIC"));
    }

    #[test]
    fn absorb_keeps_an_outcome_the_newer_poll_no_longer_reports() {
        // GameEnd lands on one poll; the game process can exit before the
        // next. The outcome has to outlive the polls that follow it.
        let mut accumulated = self_summary(&won_fixture());
        assert_eq!(accumulated.win, Some(true));

        accumulated.absorb(self_summary(&fixture()));

        assert_eq!(accumulated.win, Some(true), "the win must not be given back");
        assert_eq!(
            accumulated.champion.as_deref(),
            Some("Ahri"),
            "everything else still tracks the newest poll"
        );
    }

    #[test]
    fn absorb_takes_the_newer_kda_because_scores_only_ever_grow() {
        let mut accumulated = LiveSummary::default();
        accumulated.absorb(self_summary(&fixture()));

        let mut later = fixture();
        later.all_players[0].scores.kills = 9;
        accumulated.absorb(self_summary(&later));

        assert_eq!(accumulated.kda.unwrap().kills, 9);
    }

    // --- A possession is not a pick (#203) ---------------------------------

    /// Our fixture entry, recast as whoever `name` is, with `raw` as its
    /// `rawChampionName` (empty when the response carries none).
    fn as_champion(name: &str, raw: &str) -> AllGameData {
        let mut snapshot = fixture();
        let us = &mut snapshot.all_players[0];
        us.champion_name = name.into();
        us.raw_champion_name = raw.into();
        snapshot
    }

    /// The game #203 was found in: Viego from the start, then a possession
    /// of Malphite that the game ended during. The last poll says Malphite;
    /// the row has to say Viego.
    #[test]
    fn a_viego_who_ends_the_game_possessing_someone_is_still_viego() {
        let mut live = LiveSummary::default();
        live.absorb(self_summary(&as_champion("Viego", "game_character_displayname_Viego")));
        live.absorb(self_summary(&as_champion(
            "Malphite",
            "game_character_displayname_Malphite",
        )));

        assert_eq!(live.champion.as_deref(), Some("Viego"));
    }

    /// The same, if the key sees through the possession and only the display
    /// name changes. Which of the two a real client does is not known off
    /// Windows, and the answer must not depend on it.
    #[test]
    fn a_possession_the_key_sees_through_is_still_viego() {
        let mut live = LiveSummary::default();
        live.absorb(self_summary(&as_champion("Viego", "game_character_displayname_Viego")));
        live.absorb(self_summary(&as_champion("Malphite", "game_character_displayname_Viego")));

        assert_eq!(live.champion.as_deref(), Some("Viego"));
    }

    /// A recording that starts mid-possession, as one does after a daemon
    /// restart, first sees the possessed name. The possession ending is what
    /// corrects it.
    #[test]
    fn a_recording_that_starts_mid_possession_is_corrected_when_it_ends() {
        let mut live = LiveSummary::default();
        live.absorb(self_summary(&as_champion("Malphite", "")));
        assert_eq!(live.champion.as_deref(), Some("Malphite"), "nothing can tell yet");

        live.absorb(self_summary(&as_champion("Viego", "")));
        live.absorb(self_summary(&as_champion("Vi", "")));

        assert_eq!(live.champion.as_deref(), Some("Viego"));
    }

    /// Recognised by name when the response has no key, which is every
    /// hand-trimmed fixture and any client that stops sending it.
    #[test]
    fn viego_is_recognised_by_name_without_the_key() {
        let summary = self_summary(&as_champion("Viego", ""));
        assert!(summary.viego);
        assert_eq!(summary.champion.as_deref(), Some("Viego"));
    }

    /// Nobody else is affected: a champion that is not Viego still tracks the
    /// newest poll, as every other field does.
    #[test]
    fn settling_leaves_every_other_champion_to_the_newest_poll() {
        assert_eq!(
            settle_champion((Some("Ahri".into()), false), (Some("Lux".into()), false)),
            (Some("Lux".into()), false)
        );
        assert_eq!(
            settle_champion((Some("Ahri".into()), false), (None, false)),
            (Some("Ahri".into()), false),
            "and a poll that cannot place us gives nothing back"
        );
        assert_eq!(
            settle_champion((Some("Viego".into()), true), (None, false)),
            (Some("Viego".into()), true)
        );
        assert_eq!(
            settle_champion((None, false), (Some("Viego".into()), true)),
            (Some("Viego".into()), true)
        );
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

    /// The fixture's objectives are deliberately mixed: we kill the turret
    /// and the Baron, we assist the dragon, and the enemy takes Herald with
    /// no involvement from us at all.
    #[test]
    fn keeps_the_objectives_we_had_a_hand_in() {
        let markers = MarkerTracker::new().ingest(&fixture());
        let kinds: Vec<MarkerKind> = markers.iter().map(|m| m.kind).collect();

        assert!(kinds.contains(&MarkerKind::Turret), "we killed it");
        assert!(kinds.contains(&MarkerKind::Dragon), "we assisted it");
        assert!(kinds.contains(&MarkerKind::Baron), "we killed it");
        assert!(kinds.contains(&MarkerKind::Ace), "we closed it out");
        assert!(kinds.contains(&MarkerKind::FirstBlood), "we got it");
    }

    /// The whole point of the filter. A marker is a seek target, and an
    /// objective taken while we were on the other side of the map is a stop
    /// on `[`/`]` that shows the player nothing they were part of.
    #[test]
    fn drops_an_objective_we_took_no_part_in() {
        // Fixture Herald: killed by EnemyA, assisted by EnemyB.
        let markers = MarkerTracker::new().ingest(&fixture());
        let kinds: Vec<MarkerKind> = markers.iter().map(|m| m.kind).collect();

        assert!(
            !kinds.contains(&MarkerKind::Herald),
            "kinds were: {kinds:?}"
        );
    }

    /// Same rule for a *friendly* objective — the case that prompted this.
    /// Being on the team that took it is not taking part in it.
    #[test]
    fn a_teammates_uncontested_turret_is_not_our_marker() {
        let mut snapshot = fixture();
        snapshot.events.events.retain(|e| e.event_name == "TurretKilled");
        snapshot.events.events[0].killer_name = Some("Blitz#NA1".into());
        snapshot.events.events[0].assisters = vec![];

        assert!(MarkerTracker::new().ingest(&snapshot).is_empty());
    }

    #[test]
    fn a_multikill_of_ours_becomes_a_marker_carrying_the_streak() {
        let mut snapshot = fixture();
        snapshot.events.events = vec![GameEvent {
            event_id: 99,
            event_name: "Multikill".into(),
            event_time: 612.0,
            killer_name: Some("Ninja#NA1".into()),
            kill_streak: Some(3),
            ..blank_event()
        }];

        let markers = MarkerTracker::new().ingest(&snapshot);
        assert_eq!(markers.len(), 1);
        assert_eq!(markers[0].kind, MarkerKind::Multikill);
        assert_eq!(markers[0].payload["kill_streak"], 3);
    }

    #[test]
    fn someone_elses_multikill_is_not_our_marker() {
        let mut snapshot = fixture();
        snapshot.events.events = vec![GameEvent {
            event_id: 99,
            event_name: "Multikill".into(),
            event_time: 612.0,
            killer_name: Some("EnemyA#NA1".into()),
            kill_streak: Some(5),
            ..blank_event()
        }];

        assert!(MarkerTracker::new().ingest(&snapshot).is_empty());
    }

    /// The whole real payload, verbatim, from a live client
    /// (`fixtures/live-client/captured-allgamedata.json`).
    ///
    /// Every other live-client fixture in this repo was written by hand
    /// from Riot's docs, and that has cost real bugs: an unparseable
    /// payload ended a recording nine minutes in (#74), and `HordeKill`,
    /// `Primal Smite` and `Unleashed Teleport` were all shipped-and-broken
    /// because nothing here had seen a real one. Do not "tidy" this file to
    /// match a shape we invented — it is the shape the client sends.
    fn captured() -> AllGameData {
        let json = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../fixtures/live-client/captured-allgamedata.json"
        ));
        serde_json::from_str(json).expect("the real payload must deserialize")
    }

    /// The #74 guard. A payload the parser cannot read ends a recording, so
    /// the one payload known to have come off a real client has to parse —
    /// and every event in it has to survive `lenient_events`, since an
    /// event silently dropped there is a marker silently lost.
    #[test]
    fn the_real_payload_parses_whole() {
        let snapshot = captured();
        assert_eq!(snapshot.all_players.len(), 10);
        assert_eq!(snapshot.events.events.len(), 67, "an event was dropped as unreadable");
        assert_eq!(snapshot.game_data.game_mode, "CLASSIC");
    }

    /// `summonerName` is the champion name and the event fields use it,
    /// while `riotIdGameName` is the account name and they do not — and
    /// the other nine players are anonymised outright. This is the payload
    /// that established all of that, so it is pinned here.
    #[test]
    fn only_the_active_player_is_named_in_the_real_payload() {
        let snapshot = captured();
        let active = snapshot.active_player.as_ref().unwrap();
        assert_eq!(active.summoner_name, "Shyvana");
        assert_eq!(active.riot_id_game_name, "NinjaGoldfinch");

        let named = snapshot
            .all_players
            .iter()
            .filter(|p| !p.riot_id_game_name.is_empty())
            .count();
        assert_eq!(named, 1, "everyone but us should be anonymised");
        assert!(snapshot.all_players.iter().all(|p| !p.summoner_name.is_empty()));
    }

    /// Marker extraction over a real game, cross-checked against a number
    /// the payload states independently: the scoreboard's own KDA. If the
    /// gate or the name matching were wrong, these would disagree.
    #[test]
    fn the_real_payload_yields_markers_matching_its_own_kda() {
        let snapshot = captured();
        let mut counts: std::collections::BTreeMap<&str, usize> = Default::default();
        for marker in MarkerTracker::new().ingest(&snapshot) {
            *counts.entry(marker.kind.as_str()).or_default() += 1;
        }

        let us = snapshot
            .all_players
            .iter()
            .find(|p| p.champion_name == "Shyvana")
            .unwrap();
        assert_eq!(counts.get("kill").copied().unwrap_or(0), us.scores.kills as usize);
        assert_eq!(counts.get("death").copied().unwrap_or(0), us.scores.deaths as usize);
        assert_eq!(counts.get("assist").copied().unwrap_or(0), us.scores.assists as usize);

        // Three grubs, all ours — the event name that had no arm at all
        // until this change.
        assert_eq!(counts.get("voidgrubs").copied().unwrap_or(0), 3);

        // Objectives we took or helped take, and nothing else. The enemy
        // jungler's dragon (EventID 9) and the four turrets we were absent
        // for are gated out, as is a `FirstBlood` whose Recipient is a
        // teammate.
        assert_eq!(counts.get("dragon").copied().unwrap_or(0), 1);
        assert_eq!(counts.get("herald").copied().unwrap_or(0), 1);
        assert_eq!(counts.get("turret").copied().unwrap_or(0), 4);
        assert_eq!(counts.get("inhibitor").copied().unwrap_or(0), 1);
        assert_eq!(counts.get("multikill").copied().unwrap_or(0), 1);
        assert_eq!(counts.get("first_blood").copied().unwrap_or(0), 0);
    }

    /// The scoreboard the library row draws, over the one payload known to
    /// have come off a real client.
    ///
    /// `scoreboard_carries_every_player_with_their_items` covers the same
    /// function against the hand-written fixture, which has four players,
    /// no `summonerSpells` and no `fullRunes` — so it can prove the shape
    /// degrades but not that the full one is right. #85 listed `itemID`,
    /// `summonerSpells` and `fullRunes` as modelled from Riot's docs and
    /// never seen; this is where that stops being true.
    #[test]
    fn the_real_payload_yields_the_scoreboard_the_row_draws() {
        let board = scoreboard(&captured()).expect("the real payload has players");

        // Five a side, which is what the row's team block draws.
        assert_eq!(board.players.len(), 10);
        assert_eq!(board.players.iter().filter(|p| p.team == "ORDER").count(), 5);
        assert_eq!(board.players.iter().filter(|p| p.team == "CHAOS").count(), 5);
        assert_eq!(board.our_team.as_deref(), Some("CHAOS"));

        // The apostrophe survives, because the art layer looks the
        // champion up by exactly this string — see #85's open question
        // about `Kai'Sa`, `Nunu & Willump` and `Cho'Gath`.
        assert!(board.players.iter().any(|p| p.champion == "Kai'Sa"));

        // Positions, which are what let the row name a lane opponent rather
        // than picking one by list order. Every player in a real game has
        // one, and ours matches the `role` the same payload produces.
        assert!(
            board.players.iter().all(|p| p.position.is_some()),
            "every player in a real game has a position: {:?}",
            board.players.iter().map(|p| (&p.champion, &p.position)).collect::<Vec<_>>()
        );

        let us = board.players.iter().find(|p| p.is_us).expect("we are in this game");
        assert_eq!(us.champion, "Shyvana");

        // Exactly one opponent shares our position: that is the matchup, and
        // it is a lookup rather than a guess.
        let opponents: Vec<&ScoreboardPlayer> = board
            .players
            .iter()
            .filter(|p| p.team != us.team && p.position == us.position)
            .collect();
        assert_eq!(opponents.len(), 1, "one lane opponent, got {opponents:?}");

        // `items[].itemID`, not the price or the count the parser used to
        // be the only reader of. Slot 5 is empty in this game and slot 0
        // was sold, so the ids are the ones that were there — nothing is
        // zero-filled to make six.
        assert_eq!(us.items, vec![3078, 1001, 1037, 1036, 3340]);
        // `summonerSpells.summonerSpellOne/Two.displayName`. `Primal Smite`
        // is Smite after an upgrade, which is the name Data Dragon has no
        // entry for — see #110.
        assert_eq!(us.spells, vec!["Primal Smite", "Flash"]);

        // `fullRunes.keystone.id` and the two tree ids.
        let runes = board.our_runes.as_ref().expect("the active player has a rune page");
        assert_eq!(runes.keystone_id, 8005);
        assert_eq!(runes.keystone, "Press the Attack");
        assert_eq!(runes.primary_tree_id, 8000);
        assert_eq!(runes.secondary_tree_id, 8300);
    }

    /// `self_summary` against the same real payload.
    #[test]
    fn the_real_payload_summarises() {
        let summary = self_summary(&captured());
        assert_eq!(summary.champion.as_deref(), Some("Shyvana"));
        assert_eq!(summary.game_mode.as_deref(), Some("CLASSIC"));
        let kda = summary.kda.unwrap();
        assert_eq!((kda.kills, kda.deaths, kda.assists), (8, 1, 4));
        // Mid-game capture: no GameEnd, so no outcome yet.
        assert_eq!(summary.win, None);
    }

    /// The Voidgrubs, straight off a real ranked capture: the API files
    /// them under `HordeKill`, with `Stolen` as the string `"False"`.
    /// Nothing handled the name, so a cleared camp produced no markers at
    /// all.
    #[test]
    fn voidgrubs_we_took_become_markers() {
        let mut snapshot = fixture();
        // Three events, because the API emits one per grub.
        snapshot.events.events = (0..3)
            .map(|i| GameEvent {
                event_id: 200 + i,
                event_name: "HordeKill".into(),
                event_time: 509.0 + i as f64 * 8.0,
                killer_name: Some("Ninja#NA1".into()),
                stolen: Some(false),
                ..blank_event()
            })
            .collect();

        let markers = MarkerTracker::new().ingest(&snapshot);
        // One per grub, not one per camp — they dedupe on `EventID`, and
        // nothing here decides how far apart two grubs are still one camp.
        assert_eq!(markers.len(), 3);
        assert!(markers.iter().all(|m| m.kind == MarkerKind::Voidgrubs));
        assert_eq!(markers[0].payload["stolen"], false);
    }

    /// Gated like every other objective: the enemy jungler taking all
    /// three is not a stop on our timeline.
    #[test]
    fn the_enemy_junglers_voidgrubs_are_not_our_markers() {
        let mut snapshot = fixture();
        snapshot.events.events = vec![GameEvent {
            event_id: 210,
            event_name: "HordeKill".into(),
            event_time: 509.0,
            killer_name: Some("Sejuani".into()),
            ..blank_event()
        }];

        assert!(MarkerTracker::new().ingest(&snapshot).is_empty());
    }

    #[test]
    fn an_inhibitor_we_assisted_becomes_a_marker() {
        let mut snapshot = fixture();
        snapshot.events.events = vec![GameEvent {
            event_id: 99,
            event_name: "InhibKilled".into(),
            event_time: 1650.0,
            killer_name: Some("Blitz#NA1".into()),
            assisters: vec!["Ninja#NA1".into()],
            inhib_killed: Some("Barracks_T2_L1".into()),
            ..blank_event()
        }];

        let markers = MarkerTracker::new().ingest(&snapshot);
        assert_eq!(markers.len(), 1);
        assert_eq!(markers[0].kind, MarkerKind::Inhibitor);
        assert_eq!(markers[0].payload["inhibitor"], "Barracks_T2_L1");
    }

    /// A stolen Baron is exactly the moment someone scrubs back to find, so
    /// the flag has to survive into the payload rather than being dropped
    /// on the way through.
    #[test]
    fn a_steal_is_recorded_on_the_objective_marker() {
        let mut snapshot = fixture();
        snapshot.events.events = vec![GameEvent {
            event_id: 99,
            event_name: "BaronKill".into(),
            event_time: 1500.0,
            killer_name: Some("Ninja#NA1".into()),
            stolen: Some(true),
            ..blank_event()
        }];

        let markers = MarkerTracker::new().ingest(&snapshot);
        assert_eq!(markers[0].payload["stolen"], true);
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

    /// #199. The first poll of a recording that began mid-game carries the
    /// whole game's events, and only the ones after capture began are its.
    #[test]
    fn a_tracker_disowns_what_happened_before_capture_began() {
        let mut tracker = MarkerTracker::new();
        let snapshot = fixture();

        // Capture began at game time 500: GameStart through the turret at
        // 480 happened before it.
        assert_eq!(tracker.disown_earlier_events(&snapshot, 500.0), 8);

        let kinds: Vec<MarkerKind> = tracker.ingest(&snapshot).iter().map(|m| m.kind).collect();
        assert_eq!(
            kinds,
            vec![MarkerKind::Dragon, MarkerKind::Baron, MarkerKind::Ace],
            "the dragon at 540 onwards, and none of what came before"
        );
    }

    /// The edge is soft on purpose. An event a moment before the estimate of
    /// when capture began may well be on the video, and keeping one that
    /// is not costs a marker at 0:00, where losing one that is costs it for
    /// good.
    #[test]
    fn an_event_just_before_capture_began_is_kept() {
        let mut tracker = MarkerTracker::new();
        // First blood and its kill are at 210.5, a second and a half early.
        assert_eq!(tracker.disown_earlier_events(&fixture(), 212.0), 2);

        let kinds: Vec<MarkerKind> = tracker.ingest(&fixture()).iter().map(|m| m.kind).collect();
        assert!(kinds.contains(&MarkerKind::FirstBlood));
        assert!(kinds.contains(&MarkerKind::Kill));
    }

    /// A recording that starts on the loading screen sees a frozen clock at
    /// 0 with some capture already behind it, so the estimate is below 0 and
    /// nothing is ever earlier than that.
    #[test]
    fn a_recording_from_the_loading_screen_disowns_nothing() {
        let mut tracker = MarkerTracker::new();
        // `gameTime` 0 on a poll that landed 4s into capture.
        assert_eq!(tracker.disown_earlier_events(&fixture(), -4.0), 0);
        assert_eq!(tracker.ingest(&fixture()).len(), 8, "every marker the fixture has");
    }

    /// Only the first call counts. Everything a recording could have
    /// inherited is on its first poll's list, because the list is
    /// cumulative, so a later call has nothing to add and a clock that went
    /// backwards, or a Live Client that came back after an outage, cannot
    /// take one of the recording's own events with it.
    #[test]
    fn only_the_first_snapshot_is_disowned_from() {
        let mut tracker = MarkerTracker::new();
        let mut first = fixture();
        first.events.events.retain(|e| e.event_id <= 3);

        assert_eq!(tracker.disown_earlier_events(&first, 600.0), 4);
        assert!(tracker.ingest(&first).is_empty());

        assert_eq!(
            tracker.disown_earlier_events(&fixture(), 2000.0),
            0,
            "a second call is a no-op"
        );
        assert_eq!(
            tracker.ingest(&fixture()).len(),
            6,
            "the death at 340 onwards, all first seen after the first poll"
        );
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

    // --- AlignmentTracker ------------------------------------------------

    /// The bug this type exists for: recording begins on the first poll, so
    /// that poll sees `elapsed ~= 0` *and* the loading screen's frozen
    /// `gameTime` of 0. Measuring there yields offset 0 and puts every
    /// marker one loading screen early.
    #[test]
    fn tracker_ignores_the_loading_screens_frozen_clock() {
        let mut tracker = AlignmentTracker::new();

        // 12 seconds of loading screen, polled at 1 Hz, clock stuck at 0.
        for i in 0..12 {
            assert_eq!(
                tracker.observe(0.0, i as f64),
                None,
                "a frozen clock must not produce an alignment"
            );
        }

        // The clock ticks: 12s of video are already recorded at game time 1.
        let alignment = tracker.observe(1.0, 12.0).expect("the clock advanced");
        assert_eq!(alignment.offset_s(), 11.0);
        assert_eq!(alignment.video_time_s(0.0), 11.0, "game start is 11s in");
        assert_eq!(alignment.video_time_s(60.0), 71.0);
    }

    /// A game pause freezes `gameTime` while the video keeps rolling. A
    /// once-measured offset would put every post-pause marker early by the
    /// length of the pause; re-deriving on the next advancing poll absorbs
    /// it.
    #[test]
    fn tracker_reabsorbs_the_offset_after_a_pause() {
        let mut tracker = AlignmentTracker::new();
        tracker.observe(0.0, 5.0);
        let before = tracker.observe(1.0, 6.0).expect("the clock advanced");
        assert_eq!(before.offset_s(), 5.0);

        // 30s paused: elapsed climbs, the clock does not.
        for i in 0..30 {
            assert_eq!(
                tracker.observe(1.0, 7.0 + i as f64).map(|a| a.offset_s()),
                Some(5.0),
                "a frozen clock holds the last good offset"
            );
        }

        let after = tracker.observe(2.0, 37.0).expect("the clock advanced");
        assert_eq!(after.offset_s(), 35.0, "the pause is now in the offset");
    }

    /// Reconnecting into a game in progress: the first poll already reports
    /// a running clock, which is indistinguishable from a frozen one until
    /// it ticks. Latching one poll later costs sub-second accuracy.
    #[test]
    fn tracker_handles_a_reconnect_one_poll_late() {
        let mut tracker = AlignmentTracker::new();
        assert_eq!(tracker.observe(600.0, 0.2), None);

        let alignment = tracker.observe(601.0, 1.2).expect("the clock advanced");
        assert!((alignment.offset_s() - -599.8).abs() < 1e-9);
        assert!(
            (alignment.video_time_s(601.0) - 1.2).abs() < 1e-9,
            "game time 601 is 1.2s into this recording"
        );
    }

    /// Markers observed before the clock moved still have to land somewhere.
    #[test]
    fn tracker_falls_back_to_the_first_proven_alignment() {
        let mut tracker = AlignmentTracker::new();
        tracker.observe(0.0, 8.0);
        tracker.observe(1.0, 9.0);
        tracker.observe(400.0, 408.0);

        assert_eq!(
            tracker.fallback().offset_s(),
            8.0,
            "early markers use the first offset proven, not the latest"
        );
    }

    /// The game ended during the loading screen, so the clock never moved
    /// and no offset was ever proven. A 1:1 mapping is a guess, but keeping
    /// the markers beats dropping them.
    #[test]
    fn tracker_falls_back_to_identity_when_the_clock_never_moves() {
        let mut tracker = AlignmentTracker::new();
        tracker.observe(0.0, 3.0);
        tracker.observe(0.0, 4.0);

        assert_eq!(tracker.fallback().offset_s(), 0.0);
        assert_eq!(tracker.fallback().video_time_s(0.0), 0.0);
    }

    // --- team_diff -------------------------------------------------------
    //
    // Fixture is 2v2 rather than 5v5 on purpose: `team_diff` sums over
    // whichever players carry a given `team` string, so a 2v2 exercises the
    // identical code path as a full lobby while keeping the fixture (shared
    // with every marker test above) readable.
    //
    // Expected fixture values, ORDER = us:
    //   kills ORDER 3 + 2 = 5, CHAOS 2 + 1 = 3          diff =    +2
    //   cs    ORDER 150 + 120 = 270, CHAOS 130 + 95 = 225  diff =   +45
    //
    // Gold is deliberately absent: it is Riot's number now, arrives after
    // the game, and is tested in `lcu::timeline`.

    #[test]
    fn team_diff_is_positive_when_our_team_is_ahead() {
        let diff = team_diff(&fixture()).expect("active player is in allPlayers");
        assert_eq!(diff.our_team, "ORDER");
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
            ..Default::default()
        });

        let diff = team_diff(&snapshot).expect("EnemyA is in allPlayers");
        assert_eq!(diff.our_team, "CHAOS");
        assert_eq!(diff.kill_diff, -2);
        assert_eq!(diff.cs_diff, -45);
    }

    // --- role ---------------------------------------------------------------

    /// The vocabulary has to match `lcu::match_data::position` exactly.
    /// One column, two writers: a `role` holding both `Support` and
    /// `UTILITY` is one nothing can group by.
    #[test]
    fn live_positions_map_onto_the_words_the_column_already_holds() {
        assert_eq!(live_position("TOP").as_deref(), Some("Top"));
        assert_eq!(live_position("JUNGLE").as_deref(), Some("Jungle"));
        assert_eq!(live_position("MIDDLE").as_deref(), Some("Middle"));
        assert_eq!(live_position("BOTTOM").as_deref(), Some("Bottom"));
        // The live API says UTILITY where the LCU says BOTTOM+DUO_SUPPORT.
        // Both have to land on the same word.
        assert_eq!(live_position("UTILITY").as_deref(), Some("Support"));
        // Case and padding are the response's business, not ours.
        assert_eq!(live_position(" jungle ").as_deref(), Some("Jungle"));
    }

    /// A mode with no lanes reports an empty position, and a mode we have
    /// never seen could report anything. Neither is a role, and guessing
    /// one would put a word in a slot people read as fact.
    #[test]
    fn an_unknown_position_has_no_role_rather_than_a_guessed_one() {
        assert_eq!(live_position(""), None);
        assert_eq!(live_position("   "), None);
        assert_eq!(live_position("NEXUS_BLITZ_LANE"), None);
    }

    /// The trimmed fixture has no `position` at all, which is also what a
    /// client that stops sending it looks like.
    #[test]
    fn a_snapshot_without_positions_yields_no_role() {
        assert_eq!(self_summary(&fixture()).role, None);
    }

    #[test]
    fn our_own_position_becomes_the_role() {
        let mut snapshot = fixture();
        for player in &mut snapshot.all_players {
            player.position = if player.champion_name == "Ahri" {
                "MIDDLE".to_string()
            } else {
                "TOP".to_string()
            };
        }
        // Ours, not whoever happened to be first.
        assert_eq!(self_summary(&snapshot).role.as_deref(), Some("Middle"));
    }

    // --- scoreboard -------------------------------------------------------

    /// A transformed Gnar comes back as `Mega Gnar`, which no asset store
    /// has a portrait for — the slot rendered as an empty black square, and
    /// that is indistinguishable from art that has not loaded yet.
    #[test]
    fn a_transformed_champion_is_stored_under_its_real_name() {
        assert_eq!(normalize_champion("Mega Gnar"), "Gnar");
        assert_eq!(normalize_champion("  Mega Gnar  "), "Gnar");
    }

    /// The table must not touch anything else. `Wukong` in particular is
    /// *not* renamed here — `ddragon` maps it to `MonkeyKing` from the
    /// client's own asset store, which stays correct as champions are added.
    #[test]
    fn every_other_champion_is_left_exactly_as_it_came() {
        for name in ["Gnar", "Wukong", "Nunu & Willump", "Kai'Sa", "Shyvana"] {
            assert_eq!(normalize_champion(name), name);
        }
    }

    #[test]
    fn scoreboard_carries_every_player_with_their_items() {
        let board = scoreboard(&fixture()).expect("the fixture has players");
        assert_eq!(board.players.len(), 4);
        assert_eq!(board.our_team.as_deref(), Some("ORDER"));

        let ahri = &board.players[0];
        assert_eq!(ahri.champion, "Ahri");
        assert_eq!(ahri.team, "ORDER");
        assert_eq!(ahri.cs, 150);
        // Ids, not names: Data Dragon files item art under the id.
        assert_eq!(ahri.items, vec![3089, 3157, 2003]);
    }

    /// The row has to know which of the ten is the person watching, and
    /// that question already has an answer here (`find_us`). Marking it in
    /// the stored scoreboard is what stops the frontend growing a second
    /// one out of champion names.
    #[test]
    fn exactly_one_player_is_marked_as_us() {
        let board = scoreboard(&fixture()).unwrap();
        let ours: Vec<&ScoreboardPlayer> = board.players.iter().filter(|p| p.is_us).collect();
        assert_eq!(ours.len(), 1);
        assert_eq!(ours[0].champion, "Ahri");
    }

    /// Same refusal as `team_diff`: a snapshot we cannot place ourselves in
    /// still describes the game, but nothing in it may claim to be us.
    #[test]
    fn nobody_is_us_when_we_are_not_in_all_players() {
        let mut snapshot = fixture();
        snapshot.active_player = Some(ActivePlayer {
            riot_id_game_name: "SomeoneElse".to_string(),
            ..Default::default()
        });
        let board = scoreboard(&snapshot).unwrap();
        assert!(board.players.iter().all(|p| !p.is_us));
        assert_eq!(board.our_team, None);
    }

    /// A poll during the loading screen has no player list. Storing that
    /// would trade a real scoreboard captured earlier for the absence of
    /// one — see `RecordingSession::scoreboard`.
    #[test]
    fn a_snapshot_with_no_players_yields_no_scoreboard() {
        let mut snapshot = fixture();
        snapshot.all_players.clear();
        assert!(scoreboard(&snapshot).is_none());
    }

    /// The trimmed fixture carries no `summonerSpells` and no `fullRunes`,
    /// which is also what an older client that stops sending them looks
    /// like. Neither may fail the whole scoreboard.
    #[test]
    fn missing_spells_and_runes_are_absent_rather_than_fatal() {
        let board = scoreboard(&fixture()).unwrap();
        assert!(board.players.iter().all(|p| p.spells.is_empty()));
        assert_eq!(board.our_runes, None);
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


    // --- poll_trace -------------------------------------------------------

    #[test]
    fn a_poll_trace_reports_what_the_poll_showed() {
        let alignment = TimeAlignment::new(1512.3, 1530.0);
        assert_eq!(
            poll_trace(&fixture(), 1530.0, Some(alignment), 2),
            "game=1512.3 cap=1530.0 off=17.70 matched=yes champ=Ahri kda=3/1/2 \
             gold=450 lvl=11 events=12 new=2"
        );
    }

    /// The Practice Tool ambiguity `find_us` documents is a real recurring
    /// state, and it silently empties champion, KDA and the advantage
    /// curve — so the line has to say so rather than just going quiet.
    #[test]
    fn a_poll_we_cannot_place_ourselves_in_says_so() {
        let mut snapshot = fixture();
        snapshot.active_player = None;
        snapshot.all_players.clear();

        let line = poll_trace(&snapshot, 1530.0, None, 0);
        assert!(line.contains("matched=no"), "{line}");
        assert!(line.contains("champ=- kda=-"), "{line}");
        assert!(line.contains("gold=- lvl=-"), "{line}");
        // The game clock and the event count do not depend on finding us,
        // so they are still there — which is what makes the line useful in
        // exactly the case something has gone wrong.
        assert!(line.contains("game=1512.3"), "{line}");
        assert!(line.contains("events=12"), "{line}");
    }

    /// Before the clock has been seen to advance there is no proven
    /// offset, and reporting `0.00` would read as "aligned" rather than
    /// "not yet known".
    #[test]
    fn an_unproven_alignment_reads_as_unknown_not_as_zero() {
        let line = poll_trace(&fixture(), 4.0, None, 0);
        assert!(line.contains("off=-"), "{line}");
        assert!(!line.contains("off=0"), "{line}");
    }

    /// The greppability contract: same keys, same order, every time. A
    /// line whose shape changes with its content is one nothing can parse.
    #[test]
    fn the_key_set_is_identical_whatever_the_poll_contained() {
        let keys = |line: &str| -> Vec<String> {
            line.split_whitespace()
                .filter_map(|pair| pair.split('=').next().map(str::to_string))
                .collect()
        };

        let full = poll_trace(&fixture(), 1530.0, Some(TimeAlignment::new(1512.3, 1530.0)), 2);
        let mut empty_snapshot = fixture();
        empty_snapshot.active_player = None;
        empty_snapshot.all_players.clear();
        empty_snapshot.events.events.clear();
        let empty = poll_trace(&empty_snapshot, 0.0, None, 0);

        assert_eq!(keys(&full), keys(&empty));
        assert_eq!(
            keys(&full),
            vec!["game", "cap", "off", "matched", "champ", "kda", "gold", "lvl", "events", "new"]
        );
    }

    /// The whole point of distilling rather than keeping the payload: at
    /// 1 Hz, size is the difference between a diagnostic and a disk
    /// problem. The trimmed sample fixture is already 4.5 KB.
    #[test]
    fn a_trace_line_stays_small_enough_to_write_every_second() {
        let line = poll_trace(&fixture(), 1530.0, Some(TimeAlignment::new(1512.3, 1530.0)), 2);
        assert!(line.len() < 160, "{} bytes: {line}", line.len());
    }

    // --- lenient parsing (#74) --------------------------------------------

    /// **The #74 regression.** Riot has historically sent booleans in this
    /// API as the strings `"True"`/`"False"`, and `Stolen` rides on
    /// `DragonKill`/`HeraldKill`/`BaronKill` — events that first appear
    /// several minutes into a game. A bare `Option<bool>` rejected the
    /// value, which failed the *whole snapshot*, which the poller read as
    /// "the game is gone", which ended the recording.
    ///
    /// The whole payload must survive, not just the field.
    #[test]
    fn a_string_valued_stolen_does_not_take_the_snapshot_with_it() {
        let snapshot: AllGameData = serde_json::from_str(
            r#"{
                "gameData": {"gameTime": 549.7, "gameMode": "CLASSIC"},
                "events": {"Events": [
                    {"EventID": 1, "EventName": "GameStart", "EventTime": 0.0},
                    {"EventID": 9, "EventName": "HeraldKill", "EventTime": 549.0,
                     "KillerName": "Ninja#NA1", "Stolen": "False"}
                ]}
            }"#,
        )
        .expect("a string-valued Stolen must not fail the snapshot");

        assert_eq!(snapshot.events.events.len(), 2, "no event should be dropped");
        assert_eq!(snapshot.events.events[1].stolen, Some(false));
    }

    #[test]
    fn stolen_is_read_however_it_is_spelled() {
        let stolen = |json: &str| -> Option<bool> {
            let e: GameEvent = serde_json::from_str(&format!(
                r#"{{"EventID": 1, "EventName": "BaronKill", "EventTime": 1.0, "Stolen": {json}}}"#
            ))
            .unwrap();
            e.stolen
        };

        assert_eq!(stolen("true"), Some(true));
        assert_eq!(stolen("false"), Some(false));
        assert_eq!(stolen(r#""True""#), Some(true));
        assert_eq!(stolen(r#""False""#), Some(false));
        assert_eq!(stolen("1"), Some(true));
        assert_eq!(stolen("0"), Some(false));
        assert_eq!(stolen("null"), None);
        // Unrecognised reads as "we do not know", never as an error: not
        // knowing whether a baron was stolen is worth less than the VOD.
        assert_eq!(stolen(r#""perhaps""#), None);
    }

    #[test]
    fn kill_streak_is_read_as_a_number_or_a_string() {
        let streak = |json: &str| -> Option<i64> {
            let e: GameEvent = serde_json::from_str(&format!(
                r#"{{"EventID": 1, "EventName": "Multikill", "EventTime": 1.0, "KillStreak": {json}}}"#
            ))
            .unwrap();
            e.kill_streak
        };

        assert_eq!(streak("5"), Some(5));
        assert_eq!(streak(r#""5""#), Some(5));
        assert_eq!(streak("null"), None);
        assert_eq!(streak(r#""penta""#), None);
    }

    /// The backstop for a shape no tolerance anticipated — including an
    /// event type Riot adds after this was written. One bad entry costs
    /// that entry, not the snapshot and not the recording.
    #[test]
    fn an_unreadable_event_is_dropped_and_the_rest_survive() {
        let snapshot: AllGameData = serde_json::from_str(
            r#"{
                "gameData": {"gameTime": 100.0},
                "events": {"Events": [
                    {"EventID": 1, "EventName": "GameStart", "EventTime": 0.0},
                    {"EventID": 2, "EventTime": 50.0},
                    {"EventID": 3, "EventName": "ChampionKill", "EventTime": 60.0},
                    {"EventID": 4, "EventName": "FutureEvent", "EventTime": "not a number"}
                ]}
            }"#,
        )
        .expect("one bad event must not fail the snapshot");

        let names: Vec<&str> = snapshot
            .events
            .events
            .iter()
            .map(|e| e.event_name.as_str())
            .collect();
        assert_eq!(names, vec!["GameStart", "ChampionKill"]);
    }

    /// A snapshot whose events are entirely unreadable still parses, so
    /// the recording continues with no markers rather than ending.
    #[test]
    fn a_wholly_unreadable_event_list_still_yields_a_snapshot() {
        let snapshot: AllGameData = serde_json::from_str(
            r#"{"gameData": {"gameTime": 42.0}, "events": {"Events": [{"nope": true}]}}"#,
        )
        .unwrap();
        assert!(snapshot.events.events.is_empty());
        assert_eq!(snapshot.game_data.game_time, 42.0);
    }

    // --- Every list is lenient (#305) -------------------------------------

    /// **The #305 shape.** A list arriving as an object used to fail the
    /// whole snapshot, every poll, for the rest of the game. It now reads as
    /// its values, in numeric key order when the keys are numbers.
    #[test]
    fn a_list_that_arrives_as_an_object_is_read_as_its_values() {
        let snapshot: AllGameData = serde_json::from_str(
            r#"{
                "gameData": {"gameTime": 42.0},
                "allPlayers": [{
                    "championName": "Ahri",
                    "items": {
                        "10": {"itemID": 3340, "slot": 6},
                        "2": {"itemID": 3157, "slot": 1},
                        "0": {"itemID": 3089, "slot": 0}
                    }
                }],
                "events": {"Events": [{
                    "EventID": 1, "EventName": "ChampionKill", "EventTime": 60.0,
                    "Assisters": {"a": "Garen"}
                }]}
            }"#,
        )
        .expect("an object where a list belongs must not fail the snapshot");

        let items: Vec<i64> = snapshot.all_players[0].items.iter().map(|i| i.item_id).collect();
        // 0, 2, 10: numeric, not the string order 0, 10, 2.
        assert_eq!(items, vec![3089, 3157, 3340]);
        assert_eq!(snapshot.events.events[0].assisters, vec!["Garen".to_string()]);
    }

    /// An empty object, the likeliest form of the #305 field: `{}` where an
    /// empty inventory's `[]` was expected.
    #[test]
    fn an_empty_object_is_an_empty_list() {
        let snapshot: AllGameData = serde_json::from_str(
            r#"{"gameData": {"gameTime": 42.0}, "allPlayers": {}, "events": {"Events": {}}}"#,
        )
        .unwrap();
        assert!(snapshot.all_players.is_empty());
        assert!(snapshot.events.events.is_empty());
    }

    /// Null, a scalar, or nothing at all: empty, and the snapshot survives.
    #[test]
    fn a_null_or_scalar_list_is_empty() {
        let snapshot: AllGameData = serde_json::from_str(
            r#"{
                "gameData": {"gameTime": 42.0},
                "allPlayers": [
                    {"championName": "Ahri", "items": null},
                    {"championName": "Garen", "items": "none"}
                ],
                "events": {"Events": [
                    {"EventID": 1, "EventName": "ChampionKill", "EventTime": 60.0, "Assisters": null}
                ]}
            }"#,
        )
        .unwrap();
        assert_eq!(snapshot.all_players.len(), 2);
        assert!(snapshot.all_players.iter().all(|p| p.items.is_empty()));
        assert!(snapshot.events.events[0].assisters.is_empty());

        // No event list at all is no new markers, not an unreadable poll.
        let snapshot: AllGameData =
            serde_json::from_str(r#"{"gameData": {"gameTime": 42.0}}"#).unwrap();
        assert!(snapshot.events.events.is_empty());
        assert!(snapshot.all_players.is_empty());
    }

    /// One unreadable entry costs that entry, as it always has for events.
    #[test]
    fn an_unreadable_entry_is_skipped_not_fatal() {
        let snapshot: AllGameData = serde_json::from_str(
            r#"{
                "gameData": {"gameTime": 42.0},
                "allPlayers": [
                    {"championName": "Ahri", "items": [{"itemID": "not a number"}, {"itemID": 3089}]},
                    {"championName": "Garen", "level": "eleven"},
                    {"championName": "Zed"}
                ]
            }"#,
        )
        .unwrap();
        let names: Vec<&str> =
            snapshot.all_players.iter().map(|p| p.champion_name.as_str()).collect();
        assert_eq!(names, vec!["Ahri", "Zed"]);
        let items: Vec<i64> = snapshot.all_players[0].items.iter().map(|i| i.item_id).collect();
        assert_eq!(items, vec![3089]);
    }

    /// A stored scoreboard reads its lists the same way, so a row written by
    /// something with a different idea of the shape still loads.
    #[test]
    fn a_scoreboard_reads_its_lists_leniently() {
        let board: Scoreboard = serde_json::from_str(
            r#"{"players": [{
                "champion": "Ahri", "team": "ORDER", "level": 11,
                "kills": 3, "deaths": 1, "assists": 2, "cs": 150,
                "items": {"1": 3157, "0": 3089},
                "spells": null,
                "spell_ids": {"0": 4, "1": 14}
            }]}"#,
        )
        .unwrap();
        let player = &board.players[0];
        assert_eq!(player.items, vec![3089, 3157]);
        assert!(player.spells.is_empty());
        assert_eq!(player.spell_ids, vec![4, 14]);
    }

    /// The real fixture still reads exactly as it did.
    #[test]
    fn the_fixture_is_unchanged_by_leniency() {
        let json = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../fixtures/live-client/sample-allgamedata.json"
        ));
        let lenient: AllGameData = serde_json::from_str(json).unwrap();
        let raw: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(
            lenient.all_players.len(),
            raw["allPlayers"].as_array().unwrap().len()
        );
        assert_eq!(
            lenient.events.events.len(),
            raw["events"]["Events"].as_array().unwrap().len()
        );
        assert_eq!(
            lenient.all_players[0].items.len(),
            raw["allPlayers"][0]["items"].as_array().unwrap().len()
        );
    }
}
