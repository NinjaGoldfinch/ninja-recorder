//! Ranked standing, as the client reports it. #149.
//!
//! Two endpoints answer with the same document —
//! `/lol-ranked/v1/current-ranked-stats` for us and
//! `/lol-ranked/v1/ranked-stats/{puuid}` for anybody else — so one shape
//! covers both. Both were captured from a real client before any of this was
//! written; `fixtures/lcu/ranked-stats.json` is the trimmed result and the
//! tests below read it.
//!
//! **What is not here is as settled as what is.** The end-of-game stats block
//! carries no LP change and no tier at all — captured from a real ranked game
//! and checked — and neither ranked endpoint reports a change either; they are
//! current state. So nothing in this module computes an LP *delta*. A
//! before-and-after subtraction would be wrong across a dodge, a remake, decay,
//! a promotion series and any game played while the app was closed, and
//! DEVELOPMENT.md §5.2 already settled that argument for the gold curve: it is
//! Riot's number or it is absent.

use serde::Deserialize;

/// The queue string both the ranked endpoints and the end-of-game block use.
/// No queue-id mapping is needed anywhere — the eog block spells its queue the
/// same way this document keys it.
pub const SOLO: &str = "RANKED_SOLO_5x5";
pub const FLEX: &str = "RANKED_FLEX_SR";

/// The ladder, in order, and deliberately stopping at Master.
///
/// Master, Grandmaster and Challenger share one division and an unbounded LP
/// pool, so any number placing them *relative to each other* would be invented
/// — and #85's "no invented composite score" rule is exactly about numbers that
/// look like facts. They collapse to a single top rung: a lobby that reaches it
/// is labelled `Master+` rather than averaged past it.
pub const TIERS: &[&str] = &[
    "IRON",
    "BRONZE",
    "SILVER",
    "GOLD",
    "PLATINUM",
    "EMERALD",
    "DIAMOND",
    "MASTER",
];

/// Tiers that collapse onto `MASTER` — see `TIERS`.
const APEX: &[&str] = &["MASTER", "GRANDMASTER", "CHALLENGER"];

/// Divisions, best first. Riot numbers them the other way round (I is the
/// best), so this is the order a ladder position needs.
const DIVISIONS: &[&str] = &["IV", "III", "II", "I"];

/// One queue's standing, as the client sends it.
///
/// `queueType` is deliberately not modelled: `queueMap` is *keyed* by it, so
/// the field inside each entry only repeats the key. A struct that carries
/// values nothing reads is the thing #113's own report exists to find.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RankedEntry {
    /// `""` when the queue is unranked — **not** null. That sentinel is the
    /// whole reason `standing` exists.
    #[serde(default)]
    pub tier: String,
    /// `"NA"` when unranked, again not null.
    #[serde(default)]
    pub division: String,
    #[serde(rename = "leaguePoints", default)]
    pub league_points: i64,
}

/// The document both ranked endpoints return.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RankedStats {
    /// Keyed by queue type, which is why nothing here has to search.
    #[serde(rename = "queueMap", default)]
    pub queue_map: std::collections::HashMap<String, RankedEntry>,
}

/// The ranked ladder a queue id plays on, if it plays on one.
///
/// The eog block spells its queue this way too, so this is the only place a
/// queue *id* has to be turned into one — nothing downstream carries both.
pub fn queue_type_for(queue_id: i64) -> Option<&'static str> {
    match queue_id {
        420 => Some(SOLO),
        440 => Some(FLEX),
        _ => None,
    }
}

/// A rank we are prepared to say something about.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Standing {
    /// `"EMERALD"`, or `"MASTER"` for anything at or above it.
    pub tier: String,
    /// `None` at Master and above, where divisions do not exist.
    pub division: Option<String>,
    pub league_points: i64,
    /// Where this sits on the ladder. Comparable, and the only thing the
    /// median orders by.
    pub rung: usize,
}

impl RankedStats {
    /// This player's standing in one queue, or `None` when they have none.
    ///
    /// **The sentinels are the point.** An unranked queue arrives as
    /// `tier: ""` and `division: "NA"`, both present and both meaningless.
    /// Read into `Option<String>` they would be `Some`, and an unranked player
    /// would count as a ranked one in every average — the exact case #149's
    /// Decision 3 says to exclude. They are normalised to `None` here, at the
    /// boundary, so nothing downstream has to remember.
    pub fn standing(&self, queue_type: &str) -> Option<Standing> {
        standing_of(self.queue_map.get(queue_type)?)
    }
}

/// Pure half, so the sentinel rules are testable without a document.
pub fn standing_of(entry: &RankedEntry) -> Option<Standing> {
    let tier = entry.tier.trim().to_ascii_uppercase();
    if tier.is_empty() {
        return None;
    }
    // Apex tiers collapse; anything unknown is *not* guessed at a position.
    let tier = if APEX.contains(&tier.as_str()) {
        "MASTER".to_string()
    } else {
        tier
    };
    let rung = TIERS.iter().position(|t| *t == tier)?;

    // No divisions above Master, and `"NA"` is the unranked spelling.
    let division = match entry.division.trim().to_ascii_uppercase() {
        d if tier == "MASTER" => {
            let _ = d;
            None
        }
        d if d.is_empty() || d == "NA" => None,
        d if DIVISIONS.contains(&d.as_str()) => Some(d),
        // A division nobody recognises is dropped rather than kept: it cannot
        // be ordered, and half a rank is not a rank.
        _ => None,
    };

    Some(Standing {
        tier,
        division,
        league_points: entry.league_points,
        rung,
    })
}

/// How a standing orders against another. Tier, then division, then LP.
fn ladder_position(s: &Standing) -> (usize, usize, i64) {
    let division = s
        .division
        .as_deref()
        .and_then(|d| DIVISIONS.iter().position(|x| *x == d))
        // Master and above have no division; they sit above every division of
        // the tier below, which is what the top index gives them.
        .unwrap_or(DIVISIONS.len());
    (s.rung, division, s.league_points)
}

/// What a lobby's rank was, and how much of it was known.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct LobbyRank {
    /// The middle standing, not a mean — see `lobby_rank`.
    pub tier: String,
    pub division: Option<String>,
    /// How many of the lobby had a rank at all.
    pub known: usize,
    /// How many players were in the lobby, known or not.
    pub of: usize,
}

/// Fewer known ranks than this and the label would be a claim about a lobby
/// from a handful of its players. Half is the line: at five of ten the middle
/// value is still the middle of something.
pub const MIN_KNOWN: usize = 5;

/// The lobby's rank, as the **median** standing.
///
/// **Median, not mean, and that is what lets the ladder stop at Master.** A
/// mean needs every rank as a number, which forces an answer to "how far above
/// Diamond is Challenger" — a question with no true answer and an invented one
/// waiting. A median only ever needs an *order*, so the apex tiers can share a
/// rung without distorting anything below them. It is also the robust choice: a
/// single Iron smurf does not drag a Diamond lobby down two tiers.
///
/// Unranked players are excluded rather than counted low, and the count says
/// so. A lobby with too few known ranks gets no label at all — `None` is a
/// better answer than a confident one drawn from three people.
pub fn lobby_rank(standings: &[Option<Standing>]) -> Option<LobbyRank> {
    let of = standings.len();
    let mut known: Vec<&Standing> = standings.iter().flatten().collect();
    if known.len() < MIN_KNOWN {
        return None;
    }
    known.sort_by_key(|s| ladder_position(s));

    // The lower middle on an even count, which is the conventional choice and
    // avoids inventing a rank between two real ones.
    let middle = known[(known.len() - 1) / 2];
    Some(LobbyRank {
        tier: middle.tier.clone(),
        division: middle.division.clone(),
        known: known.len(),
        of,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn captured() -> RankedStats {
        let raw = include_str!("../../../fixtures/lcu/ranked-stats.json");
        serde_json::from_str(raw).expect("the captured fixture should parse")
    }

    fn at(tier: &str, division: &str, lp: i64) -> Option<Standing> {
        standing_of(&RankedEntry {
            tier: tier.into(),
            division: division.into(),
            league_points: lp,
        })
    }

    /// Against the real document: the ranked queue reads, and the two unranked
    /// ones are excluded by their sentinels rather than by a special case.
    #[test]
    fn the_captured_document_reads_only_the_ranked_queue() {
        let stats = captured();
        let solo = stats.standing(SOLO).expect("solo is ranked in the capture");
        assert_eq!(solo.tier, "EMERALD");
        assert_eq!(solo.division.as_deref(), Some("III"));
        assert_eq!(solo.league_points, 38);

        assert_eq!(stats.standing(FLEX), None, "empty tier means unranked");
        assert_eq!(stats.standing("RANKED_TFT"), None);
        assert_eq!(stats.standing("NOT_A_QUEUE"), None);
    }

    /// The sentinels, stated as their own rule. `""` and `"NA"` are present
    /// and meaningless, and reading them as values is the bug this prevents.
    #[test]
    fn the_empty_sentinels_are_absence_not_values() {
        assert_eq!(at("", "NA", 0), None);
        assert_eq!(at("   ", "NA", 0), None);
        assert!(at("GOLD", "NA", 12).is_some(), "an odd division must not lose the tier");
        assert_eq!(at("GOLD", "NA", 12).unwrap().division, None);
    }

    /// Apex tiers collapse onto one rung, so nothing has to say how far above
    /// Diamond a Challenger sits.
    #[test]
    fn master_and_above_share_a_rung_and_have_no_division() {
        for tier in ["MASTER", "GRANDMASTER", "CHALLENGER"] {
            let s = at(tier, "I", 500).expect("apex tiers are ranked");
            assert_eq!(s.tier, "MASTER", "{tier} collapses");
            assert_eq!(s.division, None, "{tier} has no division");
        }
        assert_eq!(at("CHALLENGER", "I", 1500).unwrap().rung, at("MASTER", "I", 0).unwrap().rung);
    }

    /// An unfamiliar tier is not guessed at a position.
    #[test]
    fn an_unknown_tier_is_not_placed() {
        assert_eq!(at("UNOBTAINIUM", "I", 0), None);
    }

    /// The ordering the median depends on, including that Riot numbers
    /// divisions the opposite way round.
    #[test]
    fn the_ladder_orders_by_tier_then_division_then_lp() {
        let mut all = vec![
            at("GOLD", "I", 0).unwrap(),
            at("IRON", "IV", 0).unwrap(),
            at("GOLD", "IV", 90).unwrap(),
            at("MASTER", "I", 200).unwrap(),
            at("GOLD", "I", 50).unwrap(),
        ];
        all.sort_by_key(ladder_position);
        let names: Vec<String> = all
            .iter()
            .map(|s| format!("{} {} {}", s.tier, s.division.as_deref().unwrap_or("-"), s.league_points))
            .collect();
        assert_eq!(
            names,
            vec![
                "IRON IV 0",
                "GOLD IV 90",
                "GOLD I 0",
                "GOLD I 50",
                "MASTER - 200",
            ]
        );
    }

    /// The headline behaviour: the middle rank, not the mean, and it survives
    /// one wild outlier.
    #[test]
    fn the_lobby_rank_is_the_middle_and_ignores_an_outlier() {
        let lobby = vec![
            at("DIAMOND", "IV", 0),
            at("DIAMOND", "II", 0),
            at("EMERALD", "I", 0),
            at("DIAMOND", "III", 0),
            at("IRON", "IV", 0),
        ];
        let rank = lobby_rank(&lobby).expect("five known");
        assert_eq!(rank.tier, "DIAMOND", "one Iron smurf must not drag it down");
        assert_eq!(rank.known, 5);
        assert_eq!(rank.of, 5);
    }

    /// Unranked players are excluded, and the label says how many it knew —
    /// "Gold II, 8 of 10" is honest in a way "Gold II" is not.
    #[test]
    fn unranked_players_are_excluded_and_counted() {
        let mut lobby: Vec<Option<Standing>> = (0..8).map(|_| at("GOLD", "II", 0)).collect();
        lobby.push(None);
        lobby.push(None);
        let rank = lobby_rank(&lobby).unwrap();
        assert_eq!((rank.known, rank.of), (8, 10));
        assert_eq!(rank.tier, "GOLD");
    }

    /// Too few known ranks is no answer rather than a confident one drawn
    /// from a handful of people.
    #[test]
    fn a_lobby_with_too_few_known_ranks_gets_no_label() {
        let mut lobby: Vec<Option<Standing>> = (0..4).map(|_| at("GOLD", "II", 0)).collect();
        lobby.resize(10, None);
        assert_eq!(lobby_rank(&lobby), None);
    }

    /// An even count takes the lower middle rather than inventing a rank
    /// between two real ones.
    #[test]
    fn an_even_count_takes_the_lower_middle() {
        let lobby = vec![
            at("SILVER", "I", 0),
            at("GOLD", "IV", 0),
            at("GOLD", "I", 0),
            at("PLATINUM", "IV", 0),
            at("SILVER", "IV", 0),
            at("PLATINUM", "I", 0),
        ];
        let rank = lobby_rank(&lobby).unwrap();
        assert_eq!((rank.tier.as_str(), rank.division.as_deref()), ("GOLD", Some("IV")));
    }

    /// Only the two ranked queues have a ladder. Everything else — normals,
    /// ARAM, Arena, a custom — has no rank to read, and guessing one would
    /// put a standing on a game that never had one.
    #[test]
    fn only_the_ranked_queues_map_to_a_ladder() {
        assert_eq!(queue_type_for(420), Some(SOLO));
        assert_eq!(queue_type_for(440), Some(FLEX));
        for other in [0, 400, 430, 450, 490, 700, 900, 1700, 1900] {
            assert_eq!(queue_type_for(other), None, "queue {other} is not ranked");
        }
    }
}
