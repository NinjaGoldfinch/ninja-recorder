//! Post-game match metadata from the LCU: win/loss, KDA, champion, queue,
//! role and patch. DEVELOPMENT.md §3.1.
//!
//! Champion *name* resolution (id → display name) is out of scope here —
//! this module only surfaces what the LCU itself returns; `champions`
//! turns the id into a name. The common path never needs even that: Live
//! Client Data writes a display name during the game, and only a game
//! whose poller never came up arrives here without one.
//!
//! Called after a finalize, not during one — see `crate::match_summary`
//! for the retry loop and the DB patch that own the timing. At the instant
//! a recording stops, the LCU is still in `WaitingForStats` and neither
//! endpoint below has an answer yet.
//!
//! ## Two endpoints, in this order
//!
//! 1. `/lol-end-of-game/v1/eog-stats-block` is *our own* stats block, not a
//!    ten-player document we have to find ourselves in. `teams[]` carries
//!    `isPlayerTeam` and `isWinningTeam`, so the outcome needs no
//!    participant join at all — which removes the most fragile step in the
//!    whole path. It is also populated during the `EndOfGame` phase,
//!    exactly when a finalize runs, so it usually answers on the first
//!    attempt.
//! 2. `/lol-match-history/v1/games/{gameId}` is authoritative and the only
//!    source for `role` and `patch`, but it is a full match document that
//!    has to be joined back to us, and it lags the end of the game.
//!
//! Whatever the first answers, the second fills the gaps in
//! (`MatchSummary::fill_gaps_from`). Neither endpoint carries a queue *id*
//! we can use — the eog block spells its queue as a string
//! (`RANKED_SOLO_5x5`) — so the `queue` column keeps coming from the
//! gameflow session captured while the game was running (`gameflow`).
//!
//! **Identifying ourselves in the match-history response is the fragile
//! part.** The participant list and the identity list are joined by
//! `participantId`, and the identity has to be matched back to us by
//! *some* account key — but which keys the endpoint actually sends has
//! changed over time, and a fixture written by hand proves nothing about
//! the wire. The LCU's own OpenAPI spec has no `puuid` on a match-history
//! participant identity at all (only `accountId`, `summonerId`,
//! `summonerName`), even though 74 other schemas in that spec do carry
//! one. So every key is optional here and `CurrentSummoner::is_me` tries
//! each in turn — a client that sends `puuid` and one that doesn't both
//! work, without needing to know which this one is.
//!
//! **None of these shapes has been seen off a real client.** Both are
//! modelled from that same spec, so every field is optional and an
//! unrecognised response degrades to "this source knew less" rather than
//! failing the fetch.

use crate::warn;
use super::client::{LcuClientError, LcuHttpClient};
use serde::{Deserialize, Serialize};

/// The LCU's `/lol-summoner/v1/current-summoner`. `displayName` has been
/// an empty string ever since Riot IDs replaced summoner names, so the
/// name now lives in `gameName` + `tagLine`; every field is optional so an
/// older (or newer) client shape still parses.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct CurrentSummoner {
    /// Optional for the same reason every other field here is: this is the
    /// shape of a client we have never met. It is also one of the three
    /// keys `is_me` joins on, so it earns its place even on a client that
    /// stops sending it.
    #[serde(default)]
    pub puuid: Option<String>,
    #[serde(rename = "summonerId", default)]
    pub summoner_id: Option<i64>,
    #[serde(rename = "accountId", default)]
    pub account_id: Option<i64>,
    #[serde(rename = "displayName", default)]
    pub display_name: Option<String>,
    #[serde(rename = "gameName", default)]
    pub game_name: Option<String>,
    #[serde(rename = "tagLine", default)]
    pub tag_line: Option<String>,
}

impl CurrentSummoner {
    /// The name to show a human: the Riot ID when we have one, otherwise
    /// the legacy display name, and `None` when the client gave us neither.
    pub fn display(&self) -> Option<String> {
        let game_name = non_empty(&self.game_name);
        match (game_name, non_empty(&self.tag_line)) {
            (Some(name), Some(tag)) => Some(format!("{}#{}", name, tag)),
            (Some(name), None) => Some(name.to_string()),
            (None, _) => non_empty(&self.display_name).map(str::to_string),
        }
    }
}

impl CurrentSummoner {
    /// Whether `identity` is us.
    ///
    /// Tries `puuid`, then `summonerId`, then `accountId`, and a key only
    /// counts when **both sides carry it**. Two absent fields are not a
    /// match: the LCU sends `summonerId: 0` for participants whose
    /// identity is hidden, and treating that as equal to a missing value
    /// would attach whichever anonymous player came first in the list.
    ///
    /// Returns false rather than guessing when nothing lines up. The
    /// caller turns that into `ParticipantNotFound`, which leaves the
    /// recording's metadata NULL — the correct outcome, since a VOD
    /// labelled with a stranger's game is worse than one labelled with
    /// nothing.
    fn is_me(&self, identity: &PlayerIdentity) -> bool {
        if let (Some(mine), Some(theirs)) = (non_empty(&self.puuid), non_empty(&identity.puuid)) {
            return mine == theirs;
        }
        if let (Some(mine), Some(theirs)) = (real_id(self.summoner_id), real_id(identity.summoner_id))
        {
            return mine == theirs;
        }
        match (real_id(self.account_id), real_id(identity.account_id)) {
            (Some(mine), Some(theirs)) => mine == theirs,
            _ => false,
        }
    }
}

fn non_empty(field: &Option<String>) -> Option<&str> {
    field.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

/// Riot's numeric account ids are always positive. Zero is what the LCU
/// puts in the slot when it will not say, so it must never join to
/// anything — including another zero.
fn real_id(field: Option<i64>) -> Option<i64> {
    field.filter(|id| *id > 0)
}

// --- `/lol-match-history/v1/games/{gameId}` ----------------------------

#[derive(Debug, Clone, Deserialize)]
struct GameParticipant {
    #[serde(rename = "championId")]
    champion_id: i64,
    #[serde(rename = "participantId")]
    participant_id: i64,
    /// Riot's side id: 100 is blue, 200 is red. Only the gold series reads
    /// it, to know which participant frames to add up. Optional because
    /// everything on this shape is — a response without it costs the gold
    /// curve and nothing else.
    #[serde(rename = "teamId", default)]
    team_id: Option<i64>,
    stats: ParticipantStats,
    /// Where `role` comes from. Optional because it is the one part of the
    /// participant this module treats as a nice-to-have — a response
    /// without a timeline still yields a usable summary.
    #[serde(default)]
    timeline: Option<ParticipantTimeline>,
}

#[derive(Debug, Clone, Deserialize)]
struct ParticipantStats {
    kills: i64,
    deaths: i64,
    assists: i64,
    win: bool,

    // Everything below is only read when a recording has no scoreboard of
    // its own — a game played before the live capture existed, or one
    // whose poller never came up. All optional: this endpoint's shape has
    // never been seen off a real client, and a field that is not there
    // costs one slot on a row rather than the whole reconstruction.
    #[serde(rename = "champLevel", default)]
    champ_level: Option<i64>,
    #[serde(rename = "totalMinionsKilled", default)]
    minions: Option<i64>,
    /// Jungle camps. Riot counts them separately, and a jungler's CS is
    /// mostly this — leaving it out would report a jungle game as having
    /// almost no farm.
    #[serde(rename = "neutralMinionsKilled", default)]
    neutral_minions: Option<i64>,
    #[serde(rename = "spell1Id", default)]
    spell1_id: Option<i64>,
    #[serde(rename = "spell2Id", default)]
    spell2_id: Option<i64>,
    /// The keystone.
    #[serde(default)]
    perk0: Option<i64>,
    #[serde(rename = "perkPrimaryStyle", default)]
    perk_primary_style: Option<i64>,
    #[serde(rename = "perkSubStyle", default)]
    perk_sub_style: Option<i64>,
    #[serde(default)]
    item0: Option<i64>,
    #[serde(default)]
    item1: Option<i64>,
    #[serde(default)]
    item2: Option<i64>,
    #[serde(default)]
    item3: Option<i64>,
    #[serde(default)]
    item4: Option<i64>,
    #[serde(default)]
    item5: Option<i64>,
    /// The trinket.
    #[serde(default)]
    item6: Option<i64>,
}

impl ParticipantStats {
    /// The inventory in slot order, with the empty slots dropped.
    ///
    /// Riot writes `0` into a slot nothing is in, and zero is not an item
    /// id — carrying it through would ask Data Dragon for `0.png`.
    fn items(&self) -> Vec<i64> {
        [
            self.item0, self.item1, self.item2, self.item3, self.item4, self.item5, self.item6,
        ]
        .into_iter()
        .flatten()
        .filter(|id| *id > 0)
        .collect()
    }

    fn spell_ids(&self) -> Vec<i64> {
        [self.spell1_id, self.spell2_id]
            .into_iter()
            .flatten()
            .filter(|id| *id > 0)
            .collect()
    }

    /// Lane minions plus jungle camps, which is what a scoreboard means by
    /// CS. `None` when the response said nothing about either.
    fn cs(&self) -> Option<i64> {
        match (self.minions, self.neutral_minions) {
            (None, None) => None,
            (a, b) => Some(a.unwrap_or(0) + b.unwrap_or(0)),
        }
    }
}

/// Riot's two-field spelling of a position: `lane` says where, `role` says
/// what — `BOTTOM` + `DUO_SUPPORT` is a support, `MIDDLE` + `SOLO` a
/// midlaner. Neither field alone is worth storing.
#[derive(Debug, Clone, Default, Deserialize)]
struct ParticipantTimeline {
    #[serde(default)]
    lane: Option<String>,
    #[serde(default)]
    role: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct ParticipantIdentity {
    #[serde(rename = "participantId")]
    participant_id: i64,
    player: PlayerIdentity,
}

/// The account keys a match-history identity can carry. All optional and
/// none guaranteed — see this module's header. `summonerName` is
/// deliberately *not* among them: display names are not unique, they
/// change, and matching on one would eventually attach a stranger's KDA
/// to somebody's VOD.
#[derive(Debug, Clone, Default, Deserialize)]
struct PlayerIdentity {
    #[serde(default)]
    puuid: Option<String>,
    #[serde(rename = "summonerId", default)]
    summoner_id: Option<i64>,
    #[serde(rename = "accountId", default)]
    account_id: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
struct GameDto {
    #[serde(rename = "gameId")]
    game_id: i64,
    #[serde(rename = "queueId")]
    queue_id: i64,
    /// `"15.3.412.9873"`. Stored whole rather than trimmed to `15.3`: the
    /// build number is what distinguishes two recordings made either side
    /// of a hotfix, and a display that wants the short form can cut it.
    #[serde(rename = "gameVersion", default)]
    game_version: Option<String>,
    /// Epoch milliseconds. Only the backfill reads it — a patch already
    /// knows which game it asked about, but a backfill is trying to work
    /// out *which* game a file holds and has nothing but the clock.
    #[serde(rename = "gameCreation", default)]
    game_creation: Option<i64>,
    /// Seconds. Same caller, same reason.
    #[serde(rename = "gameDuration", default)]
    game_duration: Option<i64>,
    participants: Vec<GameParticipant>,
    #[serde(rename = "participantIdentities")]
    participant_identities: Vec<ParticipantIdentity>,
}

/// Riot's `lane`/`role` pair reduced to the position a human would name.
///
/// Unrecognised pairs yield `None` rather than a passthrough of whatever
/// the client said. `role` is one column, and a column holding both
/// `"Support"` and `"DUO_SUPPORT"` is one that nothing can group by —
/// the same reason #54 leaves an unknown champion id NULL instead of
/// writing `"Champion 157"`.
fn position(timeline: Option<&ParticipantTimeline>) -> Option<String> {
    let timeline = timeline?;
    let lane = non_empty(&timeline.lane)?.to_ascii_uppercase();
    let role = non_empty(&timeline.role)
        .map(|r| r.to_ascii_uppercase())
        .unwrap_or_default();

    let named = match (lane.as_str(), role.as_str()) {
        ("TOP", _) => "Top",
        ("JUNGLE", _) => "Jungle",
        ("MIDDLE", _) | ("MID", _) => "Middle",
        ("BOTTOM", "DUO_SUPPORT") | ("BOT", "DUO_SUPPORT") => "Support",
        ("BOTTOM", _) | ("BOT", _) => "Bottom",
        _ => return None,
    };
    Some(named.to_string())
}

// --- `/lol-end-of-game/v1/eog-stats-block` -----------------------------

/// Our own end-of-game stats block. Everything is optional: this shape
/// comes from the LCU's OpenAPI spec rather than a captured response, and
/// a field we cannot read has to mean "ask the other endpoint", never
/// "fail the fetch".
#[derive(Debug, Clone, Default, Deserialize)]
struct EogStatsBlock {
    #[serde(rename = "gameId", default)]
    game_id: Option<i64>,
    /// Our champion, at the top level — the block is already scoped to us.
    #[serde(rename = "championId", default)]
    champion_id: Option<i64>,
    #[serde(rename = "summonerId", default)]
    summoner_id: Option<i64>,
    #[serde(default)]
    teams: Vec<EogTeam>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct EogTeam {
    #[serde(rename = "isPlayerTeam", default)]
    is_player_team: Option<bool>,
    #[serde(rename = "isWinningTeam", default)]
    is_winning_team: Option<bool>,
    #[serde(default)]
    players: Vec<EogPlayer>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct EogPlayer {
    #[serde(rename = "summonerId", default)]
    summoner_id: Option<i64>,
    /// The scoreboard, as the client's own stats vocabulary spells it.
    /// Left as a map rather than modelled: the keys are a legacy stats
    /// enum (`CHAMPIONS_KILLED`, `NUM_DEATHS`, `ASSISTS`) that this repo
    /// has never seen on the wire, and a struct would turn an unexpected
    /// spelling into a parse failure for the whole block.
    #[serde(default)]
    stats: std::collections::HashMap<String, serde_json::Value>,
}

/// One scoreboard number, under whichever of its spellings this client
/// uses, and whether it arrived as a number or a string.
fn stat(stats: &std::collections::HashMap<String, serde_json::Value>, keys: &[&str]) -> Option<i64> {
    keys.iter().find_map(|key| {
        let value = stats.get(*key)?;
        value
            .as_i64()
            .or_else(|| value.as_str().and_then(|s| s.trim().parse().ok()))
    })
}

/// Pure half of the eog fetch.
///
/// The outcome is the point: `isPlayerTeam` + `isWinningTeam` answers it
/// with no participant join at all, so it holds even on a block whose
/// player list we cannot read.
fn extract_eog(block: &EogStatsBlock) -> MatchSummary {
    let mut summary = MatchSummary {
        champion_id: block.champion_id.filter(|id| *id > 0),
        win: block
            .teams
            .iter()
            .find(|t| t.is_player_team.unwrap_or(false))
            .and_then(|t| t.is_winning_team),
        ..Default::default()
    };

    // Our row on the scoreboard, for the KDA the top level doesn't carry.
    // Matched on `summonerId` like everything else here; a block that
    // doesn't say which player is us keeps its outcome and loses only the
    // numbers, which Live Client Data has usually already written anyway.
    let me = real_id(block.summoner_id).and_then(|mine| {
        block
            .teams
            .iter()
            .flat_map(|t| t.players.iter())
            .find(|p| real_id(p.summoner_id) == Some(mine))
    });
    if let Some(me) = me {
        summary.kills = stat(&me.stats, &["CHAMPIONS_KILLED", "kills"]);
        summary.deaths = stat(&me.stats, &["NUM_DEATHS", "deaths"]);
        summary.assists = stat(&me.stats, &["ASSISTS", "assists"]);
    }
    summary
}

// --- The merged answer -------------------------------------------------

/// What the LCU could tell us about one game of ours.
///
/// Every field is optional because the two endpoints answer different
/// subsets and either may be unavailable: the eog block has the outcome
/// but no queue id or patch, match history has everything but arrives
/// late. A `None` means "this was not established", and the DB patch
/// leaves the column alone rather than nulling it.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct MatchSummary {
    pub game_id: Option<i64>,
    pub queue_id: Option<i64>,
    pub champion_id: Option<i64>,
    pub win: Option<bool>,
    pub kills: Option<i64>,
    pub deaths: Option<i64>,
    pub assists: Option<i64>,
    /// `Top` / `Jungle` / `Middle` / `Bottom` / `Support` — see `position`.
    pub role: Option<String>,
    /// `gameVersion`, e.g. `"15.3.412.9873"`.
    pub patch: Option<String>,
}

impl MatchSummary {
    /// Takes from `other` only what this summary doesn't already have.
    ///
    /// Direction matters: the eog block is fetched first *because* it is
    /// the one that needs no participant join, so where both sources
    /// answer, the one that cannot have matched the wrong player wins.
    fn fill_gaps_from(&mut self, other: MatchSummary) {
        self.game_id = self.game_id.or(other.game_id);
        self.queue_id = self.queue_id.or(other.queue_id);
        self.champion_id = self.champion_id.or(other.champion_id);
        self.win = self.win.or(other.win);
        self.kills = self.kills.or(other.kills);
        self.deaths = self.deaths.or(other.deaths);
        self.assists = self.assists.or(other.assists);
        self.role = self.role.take().or(other.role);
        self.patch = self.patch.take().or(other.patch);
    }

    /// Whether this is worth writing to a row at all. A summary that
    /// established nothing is not an answer, just a parse that succeeded.
    pub fn is_empty(&self) -> bool {
        self.win.is_none()
            && self.champion_id.is_none()
            && self.kills.is_none()
            && self.role.is_none()
            && self.patch.is_none()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum MatchDataError {
    #[error(transparent)]
    Client(#[from] LcuClientError),
    #[error("could not identify our participant in game {0}")]
    ParticipantNotFound(i64),
    #[error("the client has no stats for game {0} yet")]
    NotReady(i64),
}

/// Everything the LCU can say about `game_id`, from whichever of the two
/// endpoints answers.
///
/// `skip_match_history` is for custom games: they never reach match
/// history, so asking costs a request and a retry cycle for a 404 that
/// will never become a 200 (`gameflow::GameIdentity::is_custom`). The eog
/// block still answers for them.
///
/// Errors only when *neither* source established anything. A `NotReady`
/// is the normal answer in the seconds after a game ends and is what the
/// caller's retry loop waits out.
pub async fn fetch_match_summary(
    http: &LcuHttpClient,
    game_id: i64,
    skip_match_history: bool,
) -> Result<MatchSummary, MatchDataError> {
    // Not logged per attempt, deliberately. The caller retries this whole
    // function every few seconds, so "the block isn't ready yet" — the
    // normal answer for the first few of those — would print half a dozen
    // identical lines per game. When both sources fail, the match-history
    // error propagates below and says so once; `dev_fetch_match_summary`
    // makes a single un-retried attempt when the reason matters.
    let mut summary = fetch_eog(http, game_id).await.unwrap_or_default();

    if !skip_match_history {
        match fetch_history(http, game_id).await {
            Ok(history) => summary.fill_gaps_from(history),
            Err(e) if summary.is_empty() => return Err(e),
            // The eog block already answered, so a match history that
            // hasn't caught up only costs `role` and `patch`. Reported at
            // the level it deserves and not retried.
            Err(e) => warn!("lcu", "match history unavailable for game {game_id}: {e}"),
        }
    }

    if summary.is_empty() {
        return Err(MatchDataError::NotReady(game_id));
    }
    summary.game_id = summary.game_id.or(Some(game_id));
    Ok(summary)
}

async fn fetch_eog(http: &LcuHttpClient, game_id: i64) -> Result<MatchSummary, MatchDataError> {
    let block: EogStatsBlock = http.get_json("/lol-end-of-game/v1/eog-stats-block").await?;

    // The block is whatever game the client last showed a scoreboard for,
    // not the one we asked about. A stale one from the previous game would
    // put the wrong result on this recording, which is the single worst
    // thing this feature can do.
    if let Some(block_id) = block.game_id.filter(|id| *id > 0) {
        if block_id != game_id {
            return Err(MatchDataError::NotReady(game_id));
        }
    }

    let summary = extract_eog(&block);
    if summary.is_empty() {
        return Err(MatchDataError::NotReady(game_id));
    }
    Ok(summary)
}

// --- `/lol-match-history/v1/products/lol/current-summoner/matches` -----

/// One game from the current summoner's match history, reduced to what a
/// backfill needs: when it was played, how long it ran, and what it says
/// about us.
///
/// The summary is extracted here rather than re-fetched per game. The list
/// response carries whole game documents, so a library with forty
/// unlabelled recordings costs two requests rather than forty-two.
#[derive(Debug, Clone)]
pub struct PlayedGame {
    pub game_id: i64,
    /// Epoch milliseconds, or `None` on a client that did not say.
    pub started_at: Option<i64>,
    /// Seconds.
    pub duration_s: Option<i64>,
    pub summary: MatchSummary,
    /// Everyone in the game, for rebuilding a scoreboard on a recording
    /// that has none. Read from the same document the summary came from,
    /// so it costs no extra request.
    pub participants: Vec<ParticipantSummary>,
}

#[derive(Debug, Deserialize)]
struct MatchHistoryResponse {
    #[serde(default)]
    games: MatchHistoryGames,
}

#[derive(Debug, Default, Deserialize)]
struct MatchHistoryGames {
    /// Deliberately not `Vec<GameDto>`. This shape has never been seen off
    /// a real client, and a single game the parse cannot read — one odd
    /// queue, one field a newer client dropped — would otherwise cost the
    /// entire page and with it the whole backfill. Each entry is converted
    /// on its own below and a bad one is skipped.
    #[serde(default)]
    games: Vec<serde_json::Value>,
}

/// The current summoner's most recent `count` games, newest first.
///
/// Games we cannot find ourselves in are dropped rather than returned
/// half-filled: the backfill matches on time and would otherwise be
/// offered a candidate it could never label.
pub async fn fetch_recent_games(
    http: &LcuHttpClient,
    count: u32,
) -> Result<Vec<PlayedGame>, MatchDataError> {
    let me: CurrentSummoner = http.get_json("/lol-summoner/v1/current-summoner").await?;
    let response: MatchHistoryResponse = http
        .get_json(&format!(
            "/lol-match-history/v1/products/lol/current-summoner/matches?begIndex=0&endIndex={}",
            count
        ))
        .await?;

    Ok(response
        .games
        .games
        .into_iter()
        .filter_map(|raw| match serde_json::from_value::<GameDto>(raw) {
            Ok(game) => Some(game),
            Err(e) => {
                warn!("lcu", "skipping an unreadable match-history entry: {e}");
                None
            }
        })
        .filter_map(|game| {
            let summary = extract_summary(&me, &game).ok()?;
            Some(PlayedGame {
                game_id: game.game_id,
                started_at: game.game_creation.filter(|ms| *ms > 0),
                duration_s: game.game_duration.filter(|s| *s > 0),
                participants: participants(&me, &game),
                summary,
            })
        })
        .collect())
}

async fn fetch_history(http: &LcuHttpClient, game_id: i64) -> Result<MatchSummary, MatchDataError> {
    let me: CurrentSummoner = http.get_json("/lol-summoner/v1/current-summoner").await?;
    let game: GameDto = http
        .get_json(&format!("/lol-match-history/v1/games/{}", game_id))
        .await?;

    extract_summary(&me, &game)
}

/// Which participants were on our side of one game, and which were not.
///
/// The match timeline says what every participant had at each frame but
/// never whose side they were on, so it cannot be read without this. It
/// lives here rather than in `timeline` because working out which of ten
/// players is us is this module's job, and must keep having exactly one
/// answer.
#[derive(Debug, Clone, PartialEq)]
pub struct Sides {
    pub our_participant_id: i64,
    /// `"ORDER"` or `"CHAOS"` — Live Client Data's wording, not Riot's
    /// numeric one, because that is what `samples.our_team` already holds
    /// and one column must not carry two vocabularies.
    pub our_team: Option<String>,
    pub ours: Vec<i64>,
    pub theirs: Vec<i64>,
}

/// Riot's side ids in the wording the rest of the app uses. 100 is blue
/// side, which Live Client Data calls `ORDER`; 200 is red, which it calls
/// `CHAOS`. Anything else is not a side we can name, and `None` is the
/// only safe answer — naming it wrong inverts the sign of a whole curve.
fn team_name(team_id: i64) -> Option<String> {
    match team_id {
        100 => Some("ORDER".to_string()),
        200 => Some("CHAOS".to_string()),
        _ => None,
    }
}

fn split_sides(me: &CurrentSummoner, game: &GameDto) -> Result<Sides, MatchDataError> {
    let our_participant_id = game
        .participant_identities
        .iter()
        .find(|id| me.is_me(&id.player))
        .map(|id| id.participant_id)
        .ok_or(MatchDataError::ParticipantNotFound(game.game_id))?;

    let our_team_id = game
        .participants
        .iter()
        .find(|p| p.participant_id == our_participant_id)
        .and_then(|p| p.team_id)
        .ok_or(MatchDataError::ParticipantNotFound(game.game_id))?;

    let (ours, theirs): (Vec<_>, Vec<_>) = game
        .participants
        .iter()
        .partition(|p| p.team_id == Some(our_team_id));

    Ok(Sides {
        our_participant_id,
        our_team: team_name(our_team_id),
        ours: ours.iter().map(|p| p.participant_id).collect(),
        theirs: theirs.iter().map(|p| p.participant_id).collect(),
    })
}

/// One participant as match history describes them, before champion ids
/// have been turned into names.
///
/// The names are the caller's job: resolving one is a lookup against the
/// client's asset store (`lcu::champions`), which is async and cached, and
/// this stays a pure read of a document.
#[derive(Debug, Clone, PartialEq)]
pub struct ParticipantSummary {
    pub champion_id: i64,
    /// `"ORDER"` or `"CHAOS"`, or `None` for a side id we cannot name.
    pub team: Option<String>,
    pub is_us: bool,
    pub level: i64,
    pub kills: i64,
    pub deaths: i64,
    pub assists: i64,
    /// `None` when the response said nothing about minions at all, which
    /// is different from a game where nobody farmed.
    pub cs: Option<i64>,
    pub items: Vec<i64>,
    pub spell_ids: Vec<i64>,
    pub keystone_id: Option<i64>,
    pub primary_tree_id: Option<i64>,
    pub secondary_tree_id: Option<i64>,
}

/// Every participant in a match-history document, ours flagged.
///
/// For rebuilding a scoreboard for a recording that has none — one played
/// before the live capture existed, or one whose poller never came up.
/// Empty when we cannot find ourselves, because a scoreboard that cannot
/// say which half is ours is not one worth storing.
pub fn participants(me: &CurrentSummoner, game: &GameDto) -> Vec<ParticipantSummary> {
    let Some(our_id) = game
        .participant_identities
        .iter()
        .find(|id| me.is_me(&id.player))
        .map(|id| id.participant_id)
    else {
        return Vec::new();
    };

    game.participants
        .iter()
        .map(|p| ParticipantSummary {
            champion_id: p.champion_id,
            team: p.team_id.and_then(team_name),
            is_us: p.participant_id == our_id,
            level: p.stats.champ_level.unwrap_or(0),
            kills: p.stats.kills,
            deaths: p.stats.deaths,
            assists: p.stats.assists,
            cs: p.stats.cs(),
            items: p.stats.items(),
            spell_ids: p.stats.spell_ids(),
            keystone_id: p.stats.perk0.filter(|id| *id > 0),
            primary_tree_id: p.stats.perk_primary_style.filter(|id| *id > 0),
            secondary_tree_id: p.stats.perk_sub_style.filter(|id| *id > 0),
        })
        .collect()
}

/// Who was on our side in `game_id`, from the match-history document.
pub async fn fetch_sides(http: &LcuHttpClient, game_id: i64) -> Result<Sides, MatchDataError> {
    let me: CurrentSummoner = http.get_json("/lol-summoner/v1/current-summoner").await?;
    let game: GameDto = http
        .get_json(&format!("/lol-match-history/v1/games/{}", game_id))
        .await?;
    split_sides(&me, &game)
}

fn extract_summary(me: &CurrentSummoner, game: &GameDto) -> Result<MatchSummary, MatchDataError> {
    let my_participant_id = game
        .participant_identities
        .iter()
        .find(|id| me.is_me(&id.player))
        .map(|id| id.participant_id)
        .ok_or(MatchDataError::ParticipantNotFound(game.game_id))?;

    let participant = game
        .participants
        .iter()
        .find(|p| p.participant_id == my_participant_id)
        .ok_or(MatchDataError::ParticipantNotFound(game.game_id))?;

    Ok(MatchSummary {
        game_id: Some(game.game_id),
        queue_id: Some(game.queue_id),
        champion_id: Some(participant.champion_id),
        win: Some(participant.stats.win),
        kills: Some(participant.stats.kills),
        deaths: Some(participant.stats.deaths),
        assists: Some(participant.stats.assists),
        role: position(participant.timeline.as_ref()),
        patch: non_empty(&game.game_version).map(str::to_string),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_me() -> CurrentSummoner {
        serde_json::from_str(
            r#"{"puuid": "my-puuid", "displayName": "", "gameName": "ninja", "tagLine": "NA1"}"#,
        )
        .unwrap()
    }

    #[test]
    fn prefers_the_riot_id_over_the_hollowed_out_display_name() {
        let me: CurrentSummoner = serde_json::from_str(
            r#"{"puuid": "p", "displayName": "", "gameName": "ninja", "tagLine": "NA1"}"#,
        )
        .unwrap();
        assert_eq!(me.display().as_deref(), Some("ninja#NA1"));
    }

    #[test]
    fn falls_back_to_the_display_name_on_a_pre_riot_id_client() {
        let me: CurrentSummoner =
            serde_json::from_str(r#"{"puuid": "p", "displayName": "ninja"}"#).unwrap();
        assert_eq!(me.display().as_deref(), Some("ninja"));
    }

    #[test]
    fn has_no_name_when_the_client_gives_us_nothing_usable() {
        let me: CurrentSummoner =
            serde_json::from_str(r#"{"puuid": "p", "displayName": "  "}"#).unwrap();
        assert_eq!(me.display(), None);
    }

    fn fixture_game(json: &str) -> GameDto {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn extracts_our_stats_from_a_game_with_multiple_participants() {
        let game = fixture_game(
            r#"{
                "gameId": 555,
                "queueId": 420,
                "participants": [
                    {"championId": 1, "participantId": 1, "stats": {"kills": 1, "deaths": 9, "assists": 0, "win": false}},
                    {"championId": 99, "participantId": 2, "stats": {"kills": 7, "deaths": 2, "assists": 5, "win": true}}
                ],
                "participantIdentities": [
                    {"participantId": 1, "player": {"puuid": "someone-else"}},
                    {"participantId": 2, "player": {"puuid": "my-puuid"}}
                ]
            }"#,
        );

        let summary = extract_summary(&fixture_me(), &game).unwrap();
        assert_eq!(
            summary,
            MatchSummary {
                game_id: Some(555),
                queue_id: Some(420),
                champion_id: Some(99),
                win: Some(true),
                kills: Some(7),
                deaths: Some(2),
                assists: Some(5),
                role: None,
                patch: None,
            }
        );
    }

    /// The shape the LCU's own OpenAPI spec describes: identities carry
    /// `accountId`/`summonerId`/`summonerName` and no `puuid` at all. This
    /// used to fail at *deserialize*, not at the match — `puuid` was a
    /// required `String` — so `fetch_match_summary` would have returned a
    /// parse error for every game ever played.
    #[test]
    fn matches_on_summoner_id_when_the_response_carries_no_puuid() {
        let me: CurrentSummoner = serde_json::from_str(
            r#"{"summonerId": 42, "accountId": 7, "gameName": "ninja", "tagLine": "NA1"}"#,
        )
        .unwrap();
        let game = fixture_game(
            r#"{
                "gameId": 555,
                "queueId": 420,
                "participants": [
                    {"championId": 1, "participantId": 1, "stats": {"kills": 1, "deaths": 9, "assists": 0, "win": false}},
                    {"championId": 99, "participantId": 2, "stats": {"kills": 7, "deaths": 2, "assists": 5, "win": true}}
                ],
                "participantIdentities": [
                    {"participantId": 1, "player": {"summonerId": 11, "accountId": 12, "summonerName": "Someone"}},
                    {"participantId": 2, "player": {"summonerId": 42, "accountId": 7, "summonerName": "Ninja"}}
                ]
            }"#,
        );

        let summary = extract_summary(&me, &game).unwrap();
        assert_eq!(summary.champion_id, Some(99));
        assert_eq!(summary.win, Some(true));
    }

    /// Third key down. A client that sends neither of the first two still
    /// resolves rather than silently losing every game's metadata.
    #[test]
    fn falls_all_the_way_through_to_account_id() {
        let me: CurrentSummoner = serde_json::from_str(r#"{"accountId": 7}"#).unwrap();
        let game = fixture_game(
            r#"{
                "gameId": 555,
                "queueId": 420,
                "participants": [
                    {"championId": 99, "participantId": 2, "stats": {"kills": 7, "deaths": 2, "assists": 5, "win": true}}
                ],
                "participantIdentities": [
                    {"participantId": 2, "player": {"accountId": 7}}
                ]
            }"#,
        );

        assert_eq!(extract_summary(&me, &game).unwrap().champion_id, Some(99));
    }

    /// The LCU writes `summonerId: 0` for a participant it will not name.
    /// If zero joined to zero, the first anonymous player in the list
    /// would become "us" and their KDA would land on our VOD.
    #[test]
    fn a_zeroed_out_identity_never_matches_even_another_zero() {
        let me: CurrentSummoner =
            serde_json::from_str(r#"{"summonerId": 0, "accountId": 0}"#).unwrap();
        let game = fixture_game(
            r#"{
                "gameId": 555,
                "queueId": 420,
                "participants": [
                    {"championId": 99, "participantId": 2, "stats": {"kills": 7, "deaths": 2, "assists": 5, "win": true}}
                ],
                "participantIdentities": [
                    {"participantId": 2, "player": {"summonerId": 0, "accountId": 0}}
                ]
            }"#,
        );

        assert!(matches!(
            extract_summary(&me, &game),
            Err(MatchDataError::ParticipantNotFound(555))
        ));
    }

    /// Two people who both have a puuid and whose puuids differ are two
    /// different people, full stop. Falling through to the next key on a
    /// mismatch would let a stale or shared account id override the most
    /// authoritative identifier in the response.
    #[test]
    fn a_puuid_mismatch_does_not_fall_through_to_the_weaker_keys() {
        let me: CurrentSummoner =
            serde_json::from_str(r#"{"puuid": "mine", "summonerId": 42}"#).unwrap();
        let identity: PlayerIdentity =
            serde_json::from_str(r#"{"puuid": "theirs", "summonerId": 42}"#).unwrap();

        assert!(!me.is_me(&identity));
    }

    #[test]
    fn errors_when_our_puuid_is_not_in_the_game() {
        let game = fixture_game(
            r#"{
                "gameId": 555,
                "queueId": 420,
                "participants": [
                    {"championId": 1, "participantId": 1, "stats": {"kills": 1, "deaths": 9, "assists": 0, "win": false}}
                ],
                "participantIdentities": [
                    {"participantId": 1, "player": {"puuid": "someone-else"}}
                ]
            }"#,
        );

        assert!(matches!(
            extract_summary(&fixture_me(), &game),
            Err(MatchDataError::ParticipantNotFound(555))
        ));
    }

    // --- role -----------------------------------------------------------

    fn timeline(json: &str) -> ParticipantTimeline {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn a_bottom_lane_duo_is_told_apart_by_its_role() {
        assert_eq!(
            position(Some(&timeline(r#"{"lane": "BOTTOM", "role": "DUO_SUPPORT"}"#))),
            Some("Support".to_string())
        );
        assert_eq!(
            position(Some(&timeline(r#"{"lane": "BOTTOM", "role": "DUO_CARRY"}"#))),
            Some("Bottom".to_string())
        );
    }

    #[test]
    fn a_solo_lane_needs_no_role_at_all() {
        assert_eq!(
            position(Some(&timeline(r#"{"lane": "MIDDLE", "role": "SOLO"}"#))),
            Some("Middle".to_string())
        );
        assert_eq!(
            position(Some(&timeline(r#"{"lane": "JUNGLE"}"#))),
            Some("Jungle".to_string())
        );
    }

    /// A lane this mapping doesn't know is left NULL rather than written
    /// through raw. One column holding both `"Support"` and `"DUO_SUPPORT"`
    /// is one nothing can group by.
    #[test]
    fn an_unrecognized_lane_is_nothing_rather_than_a_passthrough() {
        assert_eq!(position(Some(&timeline(r#"{"lane": "NONE"}"#))), None);
        assert_eq!(position(Some(&timeline("{}"))), None);
        assert_eq!(position(None), None);
    }

    #[test]
    fn the_patch_comes_off_the_game_version_whole() {
        let game = fixture_game(
            r#"{
                "gameId": 555,
                "queueId": 420,
                "gameVersion": "15.3.412.9873",
                "participants": [
                    {"championId": 99, "participantId": 2, "timeline": {"lane": "TOP", "role": "SOLO"},
                     "stats": {"kills": 7, "deaths": 2, "assists": 5, "win": true}}
                ],
                "participantIdentities": [{"participantId": 2, "player": {"puuid": "my-puuid"}}]
            }"#,
        );

        let summary = extract_summary(&fixture_me(), &game).unwrap();
        assert_eq!(summary.patch.as_deref(), Some("15.3.412.9873"));
        assert_eq!(summary.role.as_deref(), Some("Top"));
    }

    // --- the end-of-game stats block ------------------------------------

    fn eog(json: &str) -> EogStatsBlock {
        serde_json::from_str(json).unwrap()
    }

    /// The reason this endpoint is tried first: the outcome falls out of
    /// two booleans, with no participant list to join ourselves against.
    #[test]
    fn the_outcome_comes_off_the_team_flags_with_no_join() {
        let block = eog(
            r#"{
                "gameId": 555,
                "championId": 99,
                "summonerId": 42,
                "teams": [
                    {"isPlayerTeam": false, "isWinningTeam": false, "players": []},
                    {"isPlayerTeam": true, "isWinningTeam": true, "players": [
                        {"summonerId": 42, "stats": {"CHAMPIONS_KILLED": 7, "NUM_DEATHS": 2, "ASSISTS": 5}}
                    ]}
                ]
            }"#,
        );

        let summary = extract_eog(&block);
        assert_eq!(summary.win, Some(true));
        assert_eq!(summary.champion_id, Some(99));
        assert_eq!(
            (summary.kills, summary.deaths, summary.assists),
            (Some(7), Some(2), Some(5))
        );
    }

    /// The stats keys are a legacy enum this repo has never seen on the
    /// wire, and the values may arrive as strings. Neither may cost us the
    /// outcome, which is the field that actually matters.
    #[test]
    fn an_unreadable_scoreboard_still_yields_the_outcome() {
        let block = eog(
            r#"{
                "gameId": 555,
                "summonerId": 42,
                "teams": [
                    {"isPlayerTeam": true, "isWinningTeam": false, "players": [
                        {"summonerId": 42, "stats": {"SOME_FUTURE_SPELLING": 7}}
                    ]}
                ]
            }"#,
        );

        let summary = extract_eog(&block);
        assert_eq!(summary.win, Some(false));
        assert_eq!(summary.kills, None);
        assert!(!summary.is_empty(), "an outcome on its own is worth writing");
    }

    #[test]
    fn scoreboard_numbers_sent_as_strings_still_parse() {
        let block = eog(
            r#"{
                "summonerId": 42,
                "teams": [{"isPlayerTeam": true, "isWinningTeam": true, "players": [
                    {"summonerId": 42, "stats": {"CHAMPIONS_KILLED": "7", "NUM_DEATHS": "2", "ASSISTS": "5"}}
                ]}]
            }"#,
        );

        let summary = extract_eog(&block);
        assert_eq!(
            (summary.kills, summary.deaths, summary.assists),
            (Some(7), Some(2), Some(5))
        );
    }

    /// A block with no player team flagged tells us nothing, and "nothing"
    /// must not read as a loss.
    #[test]
    fn a_block_that_names_no_player_team_is_empty_not_a_loss() {
        let block = eog(r#"{"teams": [{"isWinningTeam": true, "players": []}]}"#);
        let summary = extract_eog(&block);
        assert_eq!(summary.win, None);
        assert!(summary.is_empty());
    }

    #[test]
    fn an_unrecognized_block_shape_degrades_to_empty() {
        assert!(extract_eog(&eog("{}")).is_empty());
    }

    // --- merging the two sources ----------------------------------------

    /// The eog block cannot have matched the wrong player, so where both
    /// sources answer it keeps its answer; match history only fills what
    /// it left blank.
    #[test]
    fn match_history_fills_the_gaps_without_overwriting_the_eog_block() {
        let mut summary = MatchSummary {
            champion_id: Some(99),
            win: Some(true),
            kills: Some(7),
            ..Default::default()
        };
        summary.fill_gaps_from(MatchSummary {
            game_id: Some(555),
            queue_id: Some(420),
            champion_id: Some(1),
            win: Some(false),
            kills: Some(0),
            deaths: Some(9),
            assists: Some(1),
            role: Some("Middle".to_string()),
            patch: Some("15.3.412.9873".to_string()),
        });

        assert_eq!(summary.win, Some(true), "the un-joined source wins");
        assert_eq!(summary.champion_id, Some(99));
        assert_eq!(summary.kills, Some(7));
        // Everything the eog block had no answer for comes across.
        assert_eq!(summary.deaths, Some(9));
        assert_eq!(summary.queue_id, Some(420));
        assert_eq!(summary.role.as_deref(), Some("Middle"));
        assert_eq!(summary.patch.as_deref(), Some("15.3.412.9873"));
    }

    /// A summary that established nothing is not an answer. Writing one
    /// would burn the retry loop's budget on a row that gained no columns.
    #[test]
    fn a_summary_carrying_only_a_queue_id_is_still_empty() {
        let summary = MatchSummary {
            game_id: Some(555),
            queue_id: Some(420),
            ..Default::default()
        };
        assert!(summary.is_empty());
    }
}
