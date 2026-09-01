//! Post-game match metadata: champion, KDA, win/loss, queue. DEVELOPMENT.md
//! §3.1. Champion *name* resolution (id → display name, e.g. via Data
//! Dragon) is out of scope here — the VOD library UI (Phase 4/5) owns
//! that; this module only surfaces what the LCU itself returns.
//!
//! `fetch_match_summary` still isn't called anywhere — the state machine's
//! Finalizing step (Phase 3) stops short of fetching it, since reliably
//! resolving *which* gameId just finished needs LCU endpoint research this
//! machine can't verify live (no League client installed). Wired in once
//! that's confirmed and the VOD library (Phase 4) has a row to attach it
//! to. The extraction logic itself (`extract_summary`) is unit-tested
//! against fixture JSON below.
#![allow(dead_code)]

use super::client::{LcuClientError, LcuHttpClient};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize)]
pub struct CurrentSummoner {
    pub puuid: String,
    #[serde(rename = "displayName")]
    pub display_name: String,
}

#[derive(Debug, Clone, Deserialize)]
struct GameParticipant {
    #[serde(rename = "championId")]
    champion_id: i64,
    #[serde(rename = "participantId")]
    participant_id: i64,
    stats: ParticipantStats,
}

#[derive(Debug, Clone, Deserialize)]
struct ParticipantStats {
    kills: i64,
    deaths: i64,
    assists: i64,
    win: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct ParticipantIdentity {
    #[serde(rename = "participantId")]
    participant_id: i64,
    player: PlayerIdentity,
}

#[derive(Debug, Clone, Deserialize)]
struct PlayerIdentity {
    puuid: String,
}

#[derive(Debug, Clone, Deserialize)]
struct GameDto {
    #[serde(rename = "gameId")]
    game_id: i64,
    #[serde(rename = "queueId")]
    queue_id: i64,
    participants: Vec<GameParticipant>,
    #[serde(rename = "participantIdentities")]
    participant_identities: Vec<ParticipantIdentity>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MatchSummary {
    pub game_id: i64,
    pub queue_id: i64,
    pub champion_id: i64,
    pub win: bool,
    pub kills: i64,
    pub deaths: i64,
    pub assists: i64,
}

#[derive(Debug, thiserror::Error)]
pub enum MatchDataError {
    #[error(transparent)]
    Client(#[from] LcuClientError),
    #[error("could not identify our participant in game {0}")]
    ParticipantNotFound(i64),
}

/// Fetches the current summoner and the given game's data from the LCU,
/// then extracts the caller's own stats from it.
pub async fn fetch_match_summary(
    http: &LcuHttpClient,
    game_id: i64,
) -> Result<MatchSummary, MatchDataError> {
    let me: CurrentSummoner = http.get_json("/lol-summoner/v1/current-summoner").await?;
    let game: GameDto = http
        .get_json(&format!("/lol-match-history/v1/games/{}", game_id))
        .await?;

    extract_summary(&me, &game)
}

fn extract_summary(me: &CurrentSummoner, game: &GameDto) -> Result<MatchSummary, MatchDataError> {
    let my_participant_id = game
        .participant_identities
        .iter()
        .find(|id| id.player.puuid == me.puuid)
        .map(|id| id.participant_id)
        .ok_or(MatchDataError::ParticipantNotFound(game.game_id))?;

    let participant = game
        .participants
        .iter()
        .find(|p| p.participant_id == my_participant_id)
        .ok_or(MatchDataError::ParticipantNotFound(game.game_id))?;

    Ok(MatchSummary {
        game_id: game.game_id,
        queue_id: game.queue_id,
        champion_id: participant.champion_id,
        win: participant.stats.win,
        kills: participant.stats.kills,
        deaths: participant.stats.deaths,
        assists: participant.stats.assists,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_me() -> CurrentSummoner {
        serde_json::from_str(r#"{"puuid": "my-puuid", "displayName": "ninja"}"#).unwrap()
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
                game_id: 555,
                queue_id: 420,
                champion_id: 99,
                win: true,
                kills: 7,
                deaths: 2,
                assists: 5,
            }
        );
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
}
