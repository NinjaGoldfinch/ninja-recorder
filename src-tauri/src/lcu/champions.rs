//! Champion id → display name, from the running client's own asset store.
//! DEVELOPMENT.md §3.1.
//!
//! One caller: the post-game summary patch (`crate::match_summary`), for a
//! recording whose Live Client Data poller never came up and so has no
//! champion name of its own. The common path never reaches here — the live
//! API states the champion as a *name*, which is why #51 took it from
//! there in the first place.
//!
//! ## Why the client's asset store, and not Data Dragon
//!
//! What this produces has to be **byte-identical** to what Live Client
//! Data writes. `champion` is sorted on, filtered on and used as the card
//! title, so `MonkeyKing` and `Wukong` in one library is one champion in
//! two places — and the split is invisible until somebody notices half
//! their games are missing. The asset store is served by the client we are
//! already authenticated against, on the patch that client is running:
//! it cannot go stale, it needs no network, and it is up by definition
//! whenever the patch runs. Data Dragon would be an outbound dependency on
//! a remote CDN, plus a version to pin, for a cosmetic string.
//!
//! That reasoning is about *names*. Champion **art** is a different
//! question — a CDN is a perfectly good place to keep images — and
//! answering it does not change this.
//!
//! ## `alias` is the trap
//!
//! The entries carry both `name` and `alias`, and `alias` is exactly where
//! the legacy internal spellings live: `MonkeyKing` for Wukong, and the
//! punctuation-stripped `Kaisa` / `Chogath` / `Nunu`. `name` is the
//! display name and is the only field read here.
//!
//! ## Unverified
//!
//! The shape below is modelled from the LCU's OpenAPI spec, not captured
//! off a real client — same standing as `match_data`'s two endpoints.
//! Every field is optional and an unrecognised response degrades to "no
//! name", never to a wrong one. `fixtures/lcu/champion-summary.json` is
//! hand-written to that spec; `dev_lcu_get` against a live client is what
//! replaces it (a capture lands under the sanitised endpoint name, so it
//! has to be trimmed into that file by hand).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::Deserialize;

use super::client::{LcuClientError, LcuHttpClient};
use super::lockfile::LockfileInfo;
use crate::warn;

const ASSET_PATH: &str = "/lol-game-data/assets/v1/champion-summary.json";

/// Champion id → display name. A few hundred entries, and constant for as
/// long as the client runs.
pub type ChampionNames = HashMap<i64, String>;

/// One entry of `champion-summary.json`. `alias`, `squarePortraitPath` and
/// `roles` are deliberately not modelled — serde drops what it is not
/// asked for, and reading `alias` is the one mistake this module exists to
/// avoid.
#[derive(Debug, Clone, Deserialize)]
pub struct ChampionEntry {
    #[serde(default)]
    pub id: Option<i64>,
    #[serde(default)]
    pub name: Option<String>,
}

/// Builds the lookup, dropping every entry that isn't a champion.
///
/// The store carries a `{"id": -1, "name": "None"}` sentinel meaning "no
/// champion selected". Left in, it would answer the one question this
/// module is asked — "what was I playing?" — with the word "None", which
/// reads exactly like a real answer on a card. Ids at or below zero and
/// blank names are both dropped for that reason.
pub fn build_map(entries: Vec<ChampionEntry>) -> ChampionNames {
    entries
        .into_iter()
        .filter_map(|entry| {
            let id = entry.id.filter(|id| *id > 0)?;
            let name = entry.name?;
            let name = name.trim();
            if name.is_empty() {
                return None;
            }
            Some((id, name.to_string()))
        })
        .collect()
}

/// The display name for `id`, or `None` if the store has never heard of
/// it.
///
/// `None` rather than `Champion 157`: the card renders a NULL champion as
/// the game mode or the filename (`vodTitle`), which is honest about not
/// knowing. A fabricated name is not, and it would sort and filter as if
/// it were real.
pub fn resolve(names: &ChampionNames, id: i64) -> Option<String> {
    names.get(&id).cloned()
}

/// The map for one client session, fetched at most once.
///
/// Keyed by the lockfile rather than simply held forever: a client restart
/// can land on a new patch, and a table cached across it would be missing
/// the champion released that morning. Module-level because the patch is a
/// free function with no session object to hang state on, and because the
/// lifetime being cached *is* the client's, not any one caller's.
type Cached = Option<(String, Arc<ChampionNames>)>;

static CACHE: Mutex<Cached> = Mutex::new(None);

/// Which client a cached map came from. The pid alone would do — a restart
/// always gets a new one — but the port is free and rules out the reuse.
fn cache_key(lockfile: &LockfileInfo) -> String {
    format!("{}:{}", lockfile.pid, lockfile.port)
}

fn cached(key: &str) -> Option<Arc<ChampionNames>> {
    let guard = CACHE.lock().unwrap();
    match guard.as_ref() {
        Some((cached_key, names)) if cached_key == key => Some(Arc::clone(names)),
        _ => None,
    }
}

fn store(key: &str, names: Arc<ChampionNames>) {
    *CACHE.lock().unwrap() = Some((key.to_string(), names));
}

async fn fetch(client: &LcuHttpClient) -> Result<ChampionNames, LcuClientError> {
    let entries: Vec<ChampionEntry> = client.get_json(ASSET_PATH).await?;
    Ok(build_map(entries))
}

/// The display name for `id` on this client, or `None`.
///
/// **Best effort, and it must stay that way.** Win/loss, queue and KDA are
/// worth far more than a name: every failure here — no such endpoint, a
/// shape we cannot read, a client that went away between the game and the
/// patch — returns `None` so the caller writes everything else regardless.
///
/// Two patches landing at once can both miss the cache and both fetch. The
/// request is an idempotent GET of a static document, so the cost of that
/// is one duplicate request, against a lock held across an await — which
/// is not a trade worth making.
pub async fn champion_name(
    client: &LcuHttpClient,
    lockfile: &LockfileInfo,
    id: i64,
) -> Option<String> {
    let key = cache_key(lockfile);
    if let Some(names) = cached(&key) {
        return resolve(&names, id);
    }

    match fetch(client).await {
        Ok(names) => {
            let names = Arc::new(names);
            store(&key, Arc::clone(&names));
            resolve(&names, id)
        }
        Err(e) => {
            warn!("lcu", "could not read the champion asset store: {e}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Vec<ChampionEntry> {
        let json = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../fixtures/lcu/champion-summary.json"
        ));
        serde_json::from_str(json).unwrap()
    }

    /// The whole point of the module. `champion` is written by two paths
    /// and they have to agree byte for byte, so this one has to produce
    /// what Live Client Data's `championName` says — the display name —
    /// and never the internal alias sitting next to it in the same entry.
    #[test]
    fn resolves_the_display_name_not_the_alias() {
        let names = build_map(fixture());
        assert_eq!(resolve(&names, 62), Some("Wukong".to_string()));
        assert_eq!(resolve(&names, 145), Some("Kai'Sa".to_string()));
        assert_eq!(resolve(&names, 103), Some("Ahri".to_string()));
    }

    /// "None" is a real string and would render on a card as if it were a
    /// champion.
    #[test]
    fn the_no_champion_sentinel_is_not_in_the_map() {
        let names = build_map(fixture());
        assert_eq!(resolve(&names, -1), None);
        assert!(!names.values().any(|name| name == "None"));
    }

    #[test]
    fn an_unknown_id_has_no_name_rather_than_an_invented_one() {
        let names = build_map(fixture());
        assert_eq!(resolve(&names, 9999), None);
    }

    /// Every field is optional because the shape has never been seen off a
    /// real client. A response missing the ones we need has to come back
    /// as "no name", not as a panic or a blank one.
    #[test]
    fn entries_without_a_usable_id_or_name_are_dropped() {
        let entries: Vec<ChampionEntry> = serde_json::from_str(
            r#"[
                {"id": 1, "name": "Annie"},
                {"id": 2},
                {"name": "Olaf"},
                {"id": 4, "name": "   "},
                {"id": 0, "name": "Zero"},
                {}
            ]"#,
        )
        .unwrap();
        let names = build_map(entries);
        assert_eq!(names.len(), 1);
        assert_eq!(resolve(&names, 1), Some("Annie".to_string()));
    }
}
