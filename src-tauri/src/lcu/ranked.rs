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
    /// Which ladder this is a standing on.
    ///
    /// Carried rather than inferred, because a standing separated from the
    /// map it was keyed by has no other way to say — and comparing two of
    /// them is exactly where that matters: solo and flex LP are different
    /// numbers and differencing them would produce a figure about nothing.
    pub queue_type: String,
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
        standing_of(self.queue_map.get(queue_type)?, queue_type)
    }
}

/// Pure half, so the sentinel rules are testable without a document.
pub fn standing_of(entry: &RankedEntry, queue_type: &str) -> Option<Standing> {
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
        queue_type: queue_type.to_string(),
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

/// LP in a division, and divisions in a tier. Riot's own structure.
const LP_PER_DIVISION: i64 = 100;
const DIVISIONS_PER_TIER: i64 = 4;

/// Where a standing sits on the ladder as a single number, for measuring a
/// **distance** rather than an order.
///
/// Note what this is not: `ladder_position` above orders standings and is all
/// the median needs, which is exactly why the tier list could stop at Master.
/// A delta needs to know *how far*, and that is a stronger claim — so this is
/// a separate function with stricter refusals rather than a reuse of that one.
///
/// The arithmetic is not a weighting somebody chose. Four divisions of a
/// hundred LP is how the ladder is built, so `GOLD IV 98` and `GOLD III 8`
/// really are ten apart, and #85's rule against invented composite numbers is
/// not engaged by measuring a structure that already exists.
///
/// `None` in the two cases where no honest number exists:
///
/// - **Master and above**, where LP is one unbounded pool rather than four
///   hundred-point divisions. The model simply stops describing the ladder
///   there, and a number derived from it would be wrong in a way only the
///   players at the top would ever notice.
/// - **A tier with no division**, below Master. `standing_of` drops a division
///   it cannot recognise, and without one there is no telling which quarter of
///   the tier this is — a quarter being four hundred LP of uncertainty.
pub fn ladder_points(standing: &Standing) -> Option<i64> {
    if standing.tier == "MASTER" {
        return None;
    }
    let division = standing.division.as_deref()?;
    let index = DIVISIONS.iter().position(|d| *d == division)? as i64;
    let rung = standing.rung as i64;
    Some(rung * DIVISIONS_PER_TIER * LP_PER_DIVISION + index * LP_PER_DIVISION + standing.league_points)
}

/// How much LP a game moved, from two standings taken either side of it.
///
/// **This is ours, not Riot's, and the distinction is the whole reason it is
/// this careful.** Nothing the client sends reports a change; what makes this
/// sound is not arithmetic but *position*: the app reads the ladder when a
/// game starts and again when it ends, so the interval contains exactly one
/// game. The confounders that sink a naive before-and-after — another game, a
/// dodge, decay, a reading from a different day — are not mitigated here, they
/// are absent, because there is no room for them between the two readings.
///
/// It refuses rather than guesses, and each refusal is a case where a number
/// would be wrong rather than merely unknown:
///
/// - **Different ladders.** Solo and flex LP are unrelated numbers.
/// - **Either end at Master or above**, or missing a division — see
///   `ladder_points`. Crossing into Master is a change of scale, not a step.
///
/// Callers must still supply two readings that really do bracket one game;
/// that part cannot be checked here, and `match_summary::rank_still_describes`
/// is what bounds it.
pub fn lp_delta(before: &Standing, after: &Standing) -> Option<i64> {
    if before.queue_type != after.queue_type {
        return None;
    }
    Some(ladder_points(after)? - ladder_points(before)?)
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
        standing_of(
            &RankedEntry {
                tier: tier.into(),
                division: division.into(),
                league_points: lp,
            },
            SOLO,
        )
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

    // --- the delta ----------------------------------------------------

    fn solo(tier: &str, division: &str, lp: i64) -> Standing {
        at(tier, division, lp).expect("a ranked standing")
    }

    fn flex(tier: &str, division: &str, lp: i64) -> Standing {
        standing_of(
            &RankedEntry { tier: tier.into(), division: division.into(), league_points: lp },
            FLEX,
        )
        .expect("a ranked standing")
    }

    /// The ordinary case, and the one that needs no ladder at all.
    #[test]
    fn a_win_inside_one_division_is_the_lp_difference() {
        assert_eq!(lp_delta(&solo("GOLD", "IV", 40), &solo("GOLD", "IV", 60)), Some(20));
        assert_eq!(lp_delta(&solo("GOLD", "IV", 60), &solo("GOLD", "IV", 40)), Some(-20));
        assert_eq!(lp_delta(&solo("GOLD", "IV", 40), &solo("GOLD", "IV", 40)), Some(0));
    }

    /// **The case the raw subtraction gets backwards.** Gold IV 98 to Gold
    /// III 8 is a win worth ten LP; subtracting the two numbers says −90.
    #[test]
    fn a_promotion_across_a_division_is_a_gain_not_a_collapse() {
        assert_eq!(lp_delta(&solo("GOLD", "IV", 98), &solo("GOLD", "III", 8)), Some(10));
    }

    /// And the same in reverse, which the raw subtraction reads as a gain.
    #[test]
    fn a_demotion_across_a_division_is_a_loss_not_a_windfall() {
        assert_eq!(lp_delta(&solo("GOLD", "III", 8), &solo("GOLD", "IV", 92)), Some(-16));
    }

    /// A whole tier is four divisions, so crossing one is no different.
    #[test]
    fn a_promotion_across_a_tier_is_measured_the_same_way() {
        assert_eq!(lp_delta(&solo("GOLD", "I", 99), &solo("PLATINUM", "IV", 9)), Some(10));
        assert_eq!(lp_delta(&solo("PLATINUM", "IV", 9), &solo("GOLD", "I", 99)), Some(-10));
    }

    /// Distance across the whole ladder, so the tier and division weights
    /// are pinned rather than merely exercised near one boundary.
    #[test]
    fn the_ladder_is_four_hundred_lp_a_tier() {
        assert_eq!(ladder_points(&solo("IRON", "IV", 0)), Some(0));
        assert_eq!(ladder_points(&solo("IRON", "I", 99)), Some(399));
        assert_eq!(ladder_points(&solo("BRONZE", "IV", 0)), Some(400));
        assert_eq!(lp_delta(&solo("IRON", "IV", 0), &solo("BRONZE", "IV", 0)), Some(400));
    }

    /// **Unavailable at the top, and that is a decision rather than a gap.**
    /// Above Master LP is one unbounded pool, so the four-hundred-a-tier
    /// model stops describing the ladder and any number from it would be
    /// wrong in a way only those players would notice.
    #[test]
    fn nothing_is_measured_at_master_or_above() {
        assert_eq!(ladder_points(&solo("MASTER", "I", 412)), None);
        assert_eq!(lp_delta(&solo("DIAMOND", "I", 99), &solo("MASTER", "I", 0)), None);
        assert_eq!(lp_delta(&solo("MASTER", "I", 0), &solo("MASTER", "I", 60)), None);
        assert_eq!(lp_delta(&solo("MASTER", "I", 60), &solo("DIAMOND", "I", 75)), None);
        // Grandmaster and Challenger collapse onto Master, so they are
        // refused by the same rule rather than by three of them.
        assert_eq!(lp_delta(&solo("DIAMOND", "I", 99), &solo("CHALLENGER", "I", 900)), None);
    }

    /// Solo and flex LP are unrelated numbers, and differencing them would
    /// produce a figure about nothing.
    #[test]
    fn two_different_ladders_do_not_make_a_delta() {
        assert_eq!(lp_delta(&solo("GOLD", "IV", 40), &flex("GOLD", "IV", 60)), None);
        // Same ladder, still fine — the guard is the mismatch, not the field.
        assert_eq!(lp_delta(&flex("GOLD", "IV", 40), &flex("GOLD", "IV", 60)), Some(20));
    }

    /// A tier whose division could not be read is four hundred LP of
    /// uncertainty, which is not a base to measure from.
    #[test]
    fn a_standing_with_no_division_cannot_be_placed() {
        let vague = solo("GOLD", "NA", 40);
        assert_eq!(vague.division, None, "the sentinel really was dropped");
        assert_eq!(ladder_points(&vague), None);
        assert_eq!(lp_delta(&vague, &solo("GOLD", "II", 40)), None);
        assert_eq!(lp_delta(&solo("GOLD", "II", 40), &vague), None);
    }
}
