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
}
