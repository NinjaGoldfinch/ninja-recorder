//! The gold curve, from Riot's own accounting. DEVELOPMENT.md §5.
//!
//! ## What this replaces, and why nothing could be salvaged
//!
//! The review timeline's gold series used to be computed live, by summing
//! the price of the items each team was holding and adding our own unspent
//! gold. That is a *net worth in items* proxy, and the gap between it and
//! gold earned is structural rather than a calibration error:
//!
//! - **The enemy's unspent gold is invisible and ours is not.** Live Client
//!   Data has exactly one gold field and it is ours, so the estimate is
//!   biased in our favour by whatever the other team is carrying — several
//!   thousand while five of them are backing.
//! - **Sold and consumed items subtract from it but not from gold earned.**
//!   Every potion drunk and every ward placed walks the curve backwards for
//!   a player who is doing fine.
//! - **Trinkets and wards price at zero**, so support gold is under-counted
//!   on both sides, unevenly, depending on who is holding what right then.
//!
//! None of that is a coefficient away from correct. Item value is a lower
//! bound on gold earned whose deficit is unbounded, per-team and
//! time-varying. A 33-minute game read as a flat band around zero, which is
//! not what a real game looks like, and a tooltip admitting the mechanism
//! does not rescue a chart that tells somebody they were even in a game
//! they lost by eight thousand.
//!
//! `/lol-match-history/v1/game-timelines/{gameId}` carries per-participant
//! `totalGold` at each frame. That is Riot's number, after the fact, exact.
//!
//! ## What it costs
//!
//! One frame per minute instead of one sample per second — about 35 points
//! for a 35-minute game rather than 2100. That is not a real loss: the
//! renderer already buckets down to roughly a thousand points, so nobody
//! was seeing per-second gold, and per-second precision on a number wrong
//! by thousands was fake precision.
//!
//! It also means **no live gold**: nothing rendered during a recording can
//! show it, because it does not exist until the game is over. Kill and CS
//! diffs stay live and stay exact.
//!
//! **Custom and practice games get no gold curve at all**, because they
//! never reach match history. That has to render as "no gold data", never
//! as a flat zero line — a zero line reads as "you were even", which is the
//! exact failure this module exists to end.
//!
//! ## Unverified
//!
//! The frame shape is modelled from Riot's documented match timeline and
//! has not been seen off a real client. Every field is optional; a response
//! that cannot be read yields no series rather than a wrong one.

use std::collections::HashMap;

use serde::Deserialize;

use super::client::{LcuClientError, LcuHttpClient};
use super::match_data::Sides;

/// One point of the signed team gold difference.
#[derive(Debug, Clone, PartialEq)]
pub struct GoldPoint {
    /// Seconds since the game clock started, from the frame's `timestamp`.
    /// The same clock the 1 Hz samples use, so it goes through the same
    /// alignment and never a second one.
    pub game_time_s: f64,
    /// Positive means our team ahead, matching `samples.gold_diff`'s
    /// existing convention.
    pub gold_diff: f64,
}

#[derive(Debug, Default, Deserialize)]
pub struct Timeline {
    #[serde(default)]
    frames: Vec<Frame>,
}

#[derive(Debug, Default, Deserialize)]
struct Frame {
    /// Milliseconds since the game clock started.
    #[serde(default)]
    timestamp: Option<i64>,
    /// Keyed by participant id *as a string* — Riot's own JSON does that,
    /// object keys being strings — so the ids are parsed back on use.
    #[serde(rename = "participantFrames", default)]
    participant_frames: HashMap<String, ParticipantFrame>,
}

#[derive(Debug, Default, Clone, Deserialize)]
struct ParticipantFrame {
    #[serde(rename = "totalGold", default)]
    total_gold: Option<f64>,
}

/// Sums each side's `totalGold` per frame and signs the difference from
/// ours.
///
/// A frame missing a timestamp is dropped, and so is one where neither
/// side has a single readable gold figure: an unreadable frame must not
/// enter the series as a zero, because zero on this chart means "even".
pub fn gold_series(sides: &Sides, timeline: &Timeline) -> Vec<GoldPoint> {
    let mut points: Vec<GoldPoint> = timeline
        .frames
        .iter()
        .filter_map(|frame| {
            let timestamp = frame.timestamp?;
            let ours = side_total(frame, &sides.ours);
            let theirs = side_total(frame, &sides.theirs);
            let (ours, theirs) = (ours?, theirs?);
            Some(GoldPoint {
                game_time_s: timestamp as f64 / 1000.0,
                gold_diff: ours - theirs,
            })
        })
        .collect();

    // Riot sends them in order, but the series is read as a line and a
    // frame out of place would draw a spike back through the chart.
    points.sort_by(|a, b| a.game_time_s.total_cmp(&b.game_time_s));
    points
}

/// One side's gold in one frame, or `None` if the frame said nothing about
/// any of them.
fn side_total(frame: &Frame, participants: &[i64]) -> Option<f64> {
    let mut total = 0.0;
    let mut seen = false;
    for id in participants {
        if let Some(gold) = frame
            .participant_frames
            .get(&id.to_string())
            .and_then(|p| p.total_gold)
        {
            total += gold;
            seen = true;
        }
    }
    seen.then_some(total)
}

/// The signed gold series for one game, newest accounting Riot has.
pub async fn fetch_gold_series(
    http: &LcuHttpClient,
    sides: &Sides,
    game_id: i64,
) -> Result<Vec<GoldPoint>, LcuClientError> {
    let timeline: Timeline = http
        .get_json(&format!(
            "/lol-match-history/v1/game-timelines/{}",
            game_id
        ))
        .await?;
    Ok(gold_series(sides, &timeline))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sides() -> Sides {
        Sides {
            our_participant_id: 1,
            our_team: Some("ORDER".to_string()),
            ours: vec![1, 2],
            theirs: vec![6, 7],
        }
    }

    fn timeline(json: &str) -> Timeline {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn sums_each_side_and_signs_the_difference_from_ours() {
        let tl = timeline(
            r#"{"frames":[
                {"timestamp":0,"participantFrames":{
                    "1":{"totalGold":500},"2":{"totalGold":500},
                    "6":{"totalGold":500},"7":{"totalGold":500}}},
                {"timestamp":60000,"participantFrames":{
                    "1":{"totalGold":2000},"2":{"totalGold":1800},
                    "6":{"totalGold":1500},"7":{"totalGold":1000}}}
            ]}"#,
        );
        let series = gold_series(&sides(), &tl);
        assert_eq!(series.len(), 2);
        assert_eq!(series[0], GoldPoint { game_time_s: 0.0, gold_diff: 0.0 });
        assert_eq!(series[1], GoldPoint { game_time_s: 60.0, gold_diff: 1300.0 });
    }

    /// The sign is the whole point. Getting it backwards would tell
    /// somebody they were ahead in a game they lost, which is the failure
    /// this module replaced.
    #[test]
    fn behind_reads_negative() {
        let tl = timeline(
            r#"{"frames":[{"timestamp":60000,"participantFrames":{
                "1":{"totalGold":1000},"2":{"totalGold":1000},
                "6":{"totalGold":4000},"7":{"totalGold":4000}}}]}"#,
        );
        assert_eq!(gold_series(&sides(), &tl)[0].gold_diff, -6000.0);
    }

    /// A missing participant is partial data, not zero: four players'
    /// gold against five would read as a deficit that is not there. Only
    /// the participants the frame actually mentions are added, which is
    /// the honest reading of a partial frame.
    #[test]
    fn a_partial_frame_sums_only_what_it_carries() {
        let tl = timeline(
            r#"{"frames":[{"timestamp":60000,"participantFrames":{
                "1":{"totalGold":1000},"6":{"totalGold":900}}}]}"#,
        );
        assert_eq!(gold_series(&sides(), &tl)[0].gold_diff, 100.0);
    }

    /// Zero on this chart means "even". A frame that said nothing about
    /// one side must not become a point at all.
    #[test]
    fn a_frame_with_no_gold_for_a_side_is_dropped() {
        let tl = timeline(
            r#"{"frames":[
                {"timestamp":0,"participantFrames":{"1":{"totalGold":500}}},
                {"timestamp":60000,"participantFrames":{
                    "1":{"totalGold":1000},"6":{"totalGold":900}}}
            ]}"#,
        );
        let series = gold_series(&sides(), &tl);
        assert_eq!(series.len(), 1);
        assert_eq!(series[0].game_time_s, 60.0);
    }

    #[test]
    fn a_frame_without_a_timestamp_is_dropped() {
        let tl = timeline(
            r#"{"frames":[{"participantFrames":{
                "1":{"totalGold":1000},"6":{"totalGold":900}}}]}"#,
        );
        assert!(gold_series(&sides(), &tl).is_empty());
    }

    /// The series is drawn as a line, so order is not cosmetic.
    #[test]
    fn frames_come_back_in_time_order() {
        let tl = timeline(
            r#"{"frames":[
                {"timestamp":120000,"participantFrames":{"1":{"totalGold":3000},"6":{"totalGold":1000}}},
                {"timestamp":0,"participantFrames":{"1":{"totalGold":500},"6":{"totalGold":500}}},
                {"timestamp":60000,"participantFrames":{"1":{"totalGold":2000},"6":{"totalGold":1000}}}
            ]}"#,
        );
        let times: Vec<f64> = gold_series(&sides(), &tl)
            .iter()
            .map(|p| p.game_time_s)
            .collect();
        assert_eq!(times, vec![0.0, 60.0, 120.0]);
    }

    /// An empty or unreadable response yields no series, never a flat
    /// zero line.
    #[test]
    fn an_empty_timeline_yields_no_points() {
        assert!(gold_series(&sides(), &timeline(r#"{"frames":[]}"#)).is_empty());
        assert!(gold_series(&sides(), &timeline("{}")).is_empty());
    }
}
