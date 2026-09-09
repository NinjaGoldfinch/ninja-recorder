//! What the parser does **not** understand about payloads it was given.
//!
//! Fixture capture already writes every Live Client Data response to disk
//! (DEVELOPMENT.md §3.3), and nothing has ever read them back looking for
//! trouble. So an unmodelled shape surfaces when a user notices something
//! missing, or when a recording dies — `HordeKill` sat in captured payloads
//! for months while Voidgrubs silently never became markers (#113).
//!
//! **A report, not a validator.** `classify_event` returning `None` is
//! correct for most of what the API sends, and the parser's leniency is
//! deliberate (#74: a payload it could not read cost half a game). Nothing
//! here changes what is accepted. It only says what went unrecognised, so
//! somebody can decide whether that was on purpose.
//!
//! Pure, so it is testable against the captured fixture rather than against
//! a game someone has to play.

use super::events::GameEvent;
use serde::Deserialize;
use std::collections::BTreeMap;

/// Event names `classify_event` has an arm for.
///
/// **Hand-maintained, and one direction of that is guarded.** Nothing can
/// reflect over a `match`, so this list cannot be derived — but
/// `every_modelled_name_really_is_modelled` feeds each entry through
/// `classify_event` and fails if it produces nothing.
///
/// That guards the dangerous direction. A name listed here without an arm
/// would *hide* a real gap, and the test catches it. The other way round — a
/// new arm nobody added here — shows the name as unhandled, which is a false
/// positive in a report someone reads, and self-corrects the moment they do.
pub const MODELLED: &[&str] = &[
    "Ace",
    "BaronKill",
    "ChampionKill",
    "DragonKill",
    "FirstBlood",
    "HeraldKill",
    "HordeKill",
    "InhibKilled",
    "Multikill",
    "TurretKilled",
];

/// Names deliberately carrying no marker.
///
/// The distinction between "not modelled yet" and "not worth modelling"
/// cannot be inferred from the code — both look like a missing arm — so it
/// lives here. It will rot unless the report is actually read, which is the
/// honest limit of this whole idea.
pub const IGNORED: &[&str] = &[
    // The game telling us it started, which the state machine already knows.
    "GameStart",
    "GameEnd",
    // Cadence, not moments.
    "MinionsSpawning",
];

/// One name the parser did nothing with, and how often it appeared.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct UnmodelledEvent {
    pub name: String,
    pub count: usize,
}

/// Every event name in `payloads` that is neither modelled nor deliberately
/// ignored, most frequent first.
///
/// Takes raw JSON rather than `AllGameData` on purpose: a payload that fails
/// to *parse* is exactly the case worth reporting, and deserializing first
/// would throw it away before this could see it.
pub fn unmodelled_events(payloads: &[serde_json::Value]) -> Vec<UnmodelledEvent> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();

    for payload in payloads {
        let events = payload
            .get("events")
            .and_then(|e| e.get("Events"))
            .and_then(|e| e.as_array());
        for event in events.into_iter().flatten() {
            let Some(name) = event.get("EventName").and_then(|n| n.as_str()) else {
                continue;
            };
            if MODELLED.contains(&name) || IGNORED.contains(&name) {
                continue;
            }
            *counts.entry(name.to_string()).or_default() += 1;
        }
    }

    let mut found: Vec<UnmodelledEvent> = counts
        .into_iter()
        .map(|(name, count)| UnmodelledEvent { name, count })
        .collect();
    // Frequency first, then name, so the output is stable between runs and
    // the thing worth looking at is at the top.
    found.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.cmp(&b.name)));
    found
}

/// One event shape `lenient_events` threw away, and how often it was seen.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct UnreadableEvent {
    /// `EventName` when the value still carries a readable one — an event
    /// that fails on some *other* field usually does, and the name is most
    /// of what makes the failure actionable. `None` is itself a finding: an
    /// entry that cannot even say what it is.
    pub name: Option<String>,
    /// What serde objected to, which names the offending field.
    pub error: String,
    /// The event's own JSON, verbatim. The point of the whole thing: the
    /// shape is what has to be read to model it, and it is exactly what the
    /// `debug!` line was throwing away.
    pub raw: String,
    /// How many captured payloads carried this shape — see the grouping note
    /// below. Not a count of distinct events.
    pub count: usize,
}

/// Every event in `payloads` that `GameEvent` cannot deserialize.
///
/// These are the ones `lenient_events` drops to keep the rest of the
/// snapshot: the right call while a game is running (#74 is what happens
/// when one bad event takes the payload with it) and useless afterwards,
/// because all it leaves is a `debug!` line in a log nobody kept. Re-parsing
/// the captured payload reproduces the same failure exactly, so the shape is
/// recoverable after the fact rather than gone with the game.
///
/// Distinct from `unmodelled_events`, and the pair is the whole diagnosis:
/// that one lists events read perfectly well and then classified to nothing,
/// this one lists events never read at all. A name can appear in neither and
/// still be wrong, but a name in either is definitely worth a look.
pub fn unreadable_events(payloads: &[serde_json::Value]) -> Vec<UnreadableEvent> {
    // Keyed by the event's own JSON. The events array is cumulative, so one
    // bad event reappears on every poll for the rest of the game — counting
    // occurrences alone would fill the report with copies of a single shape.
    // Grouping by the raw text collapses those into one entry whose `count`
    // says how many captured payloads carried it.
    let mut found: BTreeMap<String, UnreadableEvent> = BTreeMap::new();

    for payload in payloads {
        let events = payload
            .get("events")
            .and_then(|e| e.get("Events"))
            .and_then(|e| e.as_array());
        for event in events.into_iter().flatten() {
            let Err(error) = GameEvent::deserialize(event) else {
                continue;
            };
            let raw = event.to_string();
            let entry = found.entry(raw.clone()).or_insert_with(|| UnreadableEvent {
                name: event
                    .get("EventName")
                    .and_then(|n| n.as_str())
                    .map(str::to_string),
                error: error.to_string(),
                raw,
                count: 0,
            });
            entry.count += 1;
        }
    }

    let mut found: Vec<UnreadableEvent> = found.into_values().collect();
    // Same ordering rule as `unmodelled_events`: most seen first, then a
    // stable tiebreak so the output does not shuffle between runs.
    found.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.raw.cmp(&b.raw)));
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn captured() -> serde_json::Value {
        let raw = include_str!("../../../fixtures/live-client/captured-allgamedata.json");
        serde_json::from_str(raw).expect("the captured fixture should parse")
    }

    /// The whole point, against a real payload: `FirstBrick` is genuinely
    /// unhandled and must show up, while `MinionsSpawning` and `GameStart`
    /// are ignored on purpose and must not.
    #[test]
    fn the_captured_payload_reports_only_what_is_genuinely_unhandled() {
        let found = unmodelled_events(&[captured()]);
        let names: Vec<&str> = found.iter().map(|f| f.name.as_str()).collect();

        assert!(names.contains(&"FirstBrick"), "got {names:?}");
        assert!(!names.contains(&"MinionsSpawning"), "deliberately ignored");
        assert!(!names.contains(&"GameStart"), "deliberately ignored");
        assert!(!names.contains(&"HordeKill"), "modelled since #113's example");
        assert!(!names.contains(&"ChampionKill"), "modelled");
    }

    /// Counted rather than merely listed: one stray name is a curiosity,
    /// forty is a mode nobody modelled.
    #[test]
    fn repeated_names_are_counted_and_the_loudest_comes_first() {
        let payload = serde_json::json!({
            "events": { "Events": [
                { "EventName": "Rare" },
                { "EventName": "Common" },
                { "EventName": "Common" },
                { "EventName": "ChampionKill" },
            ]}
        });
        let found = unmodelled_events(&[payload]);
        assert_eq!(
            found,
            vec![
                UnmodelledEvent { name: "Common".into(), count: 2 },
                UnmodelledEvent { name: "Rare".into(), count: 1 },
            ]
        );
    }

    /// A payload shaped nothing like the API's must not panic — the reason
    /// this takes raw JSON is to see malformed input, so it has to survive it.
    #[test]
    fn a_payload_with_no_events_at_all_is_not_an_error() {
        assert!(unmodelled_events(&[serde_json::json!({})]).is_empty());
        assert!(unmodelled_events(&[serde_json::json!({ "events": 7 })]).is_empty());
        assert!(unmodelled_events(&[]).is_empty());
    }

    /// The guard described on `MODELLED`. A name listed there without a real
    /// arm would silently hide a gap, which is the failure that matters.
    #[test]
    fn every_modelled_name_really_is_modelled() {
        for name in MODELLED {
            let payload = serde_json::json!({
                "events": { "Events": [{ "EventName": name }] }
            });
            assert!(
                unmodelled_events(&[payload]).is_empty(),
                "{name} is listed as modelled"
            );
        }
    }

    /// The two lists must not overlap, or a name's meaning depends on which
    /// is checked first.
    #[test]
    fn nothing_is_both_modelled_and_ignored() {
        for name in IGNORED {
            assert!(!MODELLED.contains(name), "{name} is in both lists");
        }
    }

    /// The captured real payload deserializes cleanly, so the report is
    /// empty on it. Worth asserting: a function that always finds something
    /// would be useless, and this pins that the clean case is clean.
    #[test]
    fn the_captured_payload_has_no_unreadable_events() {
        assert_eq!(unreadable_events(&[captured()]), vec![]);
    }

    /// The shape #74 was actually about: a field present with the wrong
    /// *type*. `default` does not cover this — it only covers a missing key
    /// — so the entry fails and `lenient_events` drops it.
    #[test]
    fn an_event_with_a_wrong_typed_field_is_reported_with_its_json() {
        let payload = serde_json::json!({
            "events": { "Events": [
                { "EventID": "not-a-number", "EventName": "ChampionKill", "EventTime": 61.0 }
            ]}
        });
        let found = unreadable_events(&[payload]);
        assert_eq!(found.len(), 1, "got {found:?}");
        assert_eq!(found[0].name.as_deref(), Some("ChampionKill"));
        assert_eq!(found[0].count, 1);
        // The raw JSON is the deliverable — without it the report says
        // something is wrong and not what.
        assert!(found[0].raw.contains("not-a-number"), "got {}", found[0].raw);
        assert!(!found[0].error.is_empty());
    }

    /// The events array is cumulative, so one bad event reappears on every
    /// poll. It must collapse to a single entry rather than filling the
    /// report with copies of itself.
    #[test]
    fn the_same_bad_event_across_polls_collapses_to_one_entry() {
        let poll = serde_json::json!({
            "events": { "Events": [
                { "EventID": 1, "EventName": "GameStart", "EventTime": 0.0 },
                { "EventID": "bad", "EventName": "DragonKill", "EventTime": 900.0 }
            ]}
        });
        let found = unreadable_events(&[poll.clone(), poll.clone(), poll]);
        assert_eq!(found.len(), 1, "one shape, not three: {found:?}");
        assert_eq!(found[0].count, 3, "count says how many payloads carried it");
    }

    /// An entry that cannot even say what it is still has to be reported —
    /// it is the least readable thing the API could send and the most worth
    /// seeing.
    #[test]
    fn an_event_with_no_readable_name_is_still_reported() {
        let payload = serde_json::json!({ "events": { "Events": [ { "EventTime": 12.0 } ] } });
        let found = unreadable_events(&[payload]);
        assert_eq!(found.len(), 1, "got {found:?}");
        assert_eq!(found[0].name, None);
    }

    /// Two different broken shapes are two findings, ordered by how often
    /// each was seen so the common one leads.
    #[test]
    fn distinct_shapes_are_separate_findings_most_seen_first() {
        let a = serde_json::json!({ "events": { "Events": [
            { "EventID": "x", "EventName": "Ace", "EventTime": 1.0 } ]}});
        let b = serde_json::json!({ "events": { "Events": [
            { "EventID": "y", "EventName": "BaronKill", "EventTime": 2.0 } ]}});
        let found = unreadable_events(&[a.clone(), b, a]);
        assert_eq!(found.len(), 2, "got {found:?}");
        assert_eq!(found[0].name.as_deref(), Some("Ace"), "seen twice, leads");
        assert_eq!(found[0].count, 2);
        assert_eq!(found[1].count, 1);
    }
}
