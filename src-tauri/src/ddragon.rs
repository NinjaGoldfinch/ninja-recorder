//! Champion art from Data Dragon, cached on disk, bundled with nothing.
//! DEVELOPMENT.md §5.3.
//!
//! ## Why a CDN here, when #54 refused one
//!
//! `lcu::champions` resolves a champion *name* and deliberately does not use
//! Data Dragon: a name has to be byte-identical to what Live Client Data
//! writes, the client is up by definition when that runs, and a remote
//! dependency for a string would buy nothing. **Art is the opposite case.**
//! It is not a value anything sorts or filters on, the client is usually
//! *not* running when somebody browses their library, and Riot publishes it
//! on a CDN precisely so applications do not ship it.
//!
//! ## The weight budget
//!
//! Nothing is bundled. Files are fetched the first time a champion appears
//! and cached under `<app data>/ddragon/<version>/`, so the installer grows
//! by zero and the disk cost is what the user actually played — a champion
//! square is about 7 KB, so a library touching sixty of them is under half a
//! megabyte.
//!
//! ## Offline is the normal case, not the edge case
//!
//! This is a local VOD library. People open it with League closed and
//! sometimes with no network at all, so **every failure here returns `None`
//! and the card renders the text it always did**. A missing icon is never an
//! error, never a toast and never a broken image.
//!
//! The version is resolved once a day at most and written beside the cache,
//! so a session with no network reuses the last known one rather than
//! failing outright. Art is *not* pinned to each recording's own patch: a
//! game played on 15.16 drawn with 15.17 icons is not a problem worth a
//! cache generation per patch.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use serde::Deserialize;

use crate::warn;

const CDN: &str = "https://ddragon.leagueoflegends.com";

/// How long a resolved version is trusted before asking again. Riot ships a
/// patch every couple of weeks; a day is far inside that and keeps the
/// request count to one per session in practice.
const VERSION_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// Data Dragon's `champion.json`, reduced to the one thing art needs.
#[derive(Debug, Deserialize)]
struct ChampionData {
    #[serde(default)]
    data: HashMap<String, ChampionEntry>,
}

#[derive(Debug, Deserialize)]
struct ChampionEntry {
    /// The key art is filed under — `MonkeyKing`, not `Wukong`. Data
    /// Dragon's own field for this is called `id`, which is *not* the
    /// numeric champion id; that one is `key`, as a string.
    #[serde(default)]
    id: Option<String>,
    /// The display name, which is what `recordings.champion` holds.
    #[serde(default)]
    name: Option<String>,
}

/// Display name → the key its art is filed under.
type ArtKeys = HashMap<String, String>;

type Cached = Option<(String, Arc<ArtKeys>)>;

/// Resolved once per process. Two callers racing costs a duplicate request
/// for a static document, which is a better trade than holding a lock
/// across an await.
static CACHE: Mutex<Cached> = Mutex::new(None);

fn client() -> Option<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .ok()
}

/// The newest published version, from disk if it was resolved recently.
///
/// A stale cached version is preferred over no version at all: art from
/// last patch is not wrong in any way a person would notice, and an
/// unreachable CDN must not empty the library's icons.
async fn version(dir: &Path) -> Option<String> {
    let stamp = dir.join("version.txt");
    let cached = std::fs::read_to_string(&stamp)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty());

    let fresh = std::fs::metadata(&stamp)
        .and_then(|m| m.modified())
        .map(|t| SystemTime::now().duration_since(t).unwrap_or_default() < VERSION_TTL)
        .unwrap_or(false);

    if fresh {
        if let Some(cached) = cached {
            return Some(cached);
        }
    }

    match fetch_latest_version().await {
        Some(version) => {
            let _ = std::fs::create_dir_all(dir);
            let _ = std::fs::write(&stamp, &version);
            Some(version)
        }
        // Offline, or Riot moved the endpoint. Whatever was there last is
        // still a better answer than none.
        None => cached,
    }
}

async fn fetch_latest_version() -> Option<String> {
    let versions: Vec<String> = client()?
        .get(format!("{CDN}/api/versions.json"))
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .json()
        .await
        .ok()?;
    // Newest first, which is Data Dragon's own ordering.
    versions.into_iter().next()
}

/// Display name → art key, cached on disk per version and in memory per
/// process.
async fn art_keys(dir: &Path, version: &str) -> Option<Arc<ArtKeys>> {
    if let Some((cached_version, keys)) = CACHE.lock().unwrap().as_ref() {
        if cached_version == version {
            return Some(Arc::clone(keys));
        }
    }

    let path = dir.join(version).join("champion.json");
    let body = match std::fs::read_to_string(&path) {
        Ok(body) => body,
        Err(_) => {
            let body = client()?
                .get(format!("{CDN}/cdn/{version}/data/en_US/champion.json"))
                .send()
                .await
                .ok()?
                .error_for_status()
                .ok()?
                .text()
                .await
                .ok()?;
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&path, &body);
            body
        }
    };

    let parsed: ChampionData = serde_json::from_str(&body)
        .map_err(|e| warn!("ddragon", "champion.json did not parse: {e}"))
        .ok()?;

    let keys: ArtKeys = parsed
        .data
        .into_values()
        .filter_map(|entry| Some((entry.name?, entry.id?)))
        .collect();

    let keys = Arc::new(keys);
    *CACHE.lock().unwrap() = Some((version.to_string(), Arc::clone(&keys)));
    Some(keys)
}

/// The cached square portrait for a champion display name, fetching it if
/// this is the first time it has been asked for.
///
/// `None` for everything that could go wrong — no network, an unknown
/// champion, an unwritable cache — because the caller's fallback is the
/// text that was on the card before any of this existed.
pub async fn champion_icon(dir: &Path, champion: &str) -> Option<PathBuf> {
    let version = version(dir).await?;
    let key = art_keys(dir, &version).await?.get(champion)?.clone();

    let path = dir.join(&version).join("champion").join(format!("{key}.png"));
    if path.is_file() {
        return Some(path);
    }

    let bytes = client()?
        .get(format!("{CDN}/cdn/{version}/img/champion/{key}.png"))
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .bytes()
        .await
        .ok()?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok()?;
    }
    std::fs::write(&path, &bytes).ok()?;
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole reason this file exists rather than the frontend building
    /// a URL from `recordings.champion`: art is filed under the key, and
    /// the key is not the name. `MonkeyKing.png` is Wukong's portrait.
    #[test]
    fn maps_display_names_onto_the_key_art_is_filed_under() {
        let parsed: ChampionData = serde_json::from_str(
            r#"{"data":{
                "MonkeyKing":{"id":"MonkeyKing","key":"62","name":"Wukong"},
                "Kaisa":{"id":"Kaisa","key":"145","name":"Kai'Sa"},
                "Ahri":{"id":"Ahri","key":"103","name":"Ahri"}
            }}"#,
        )
        .unwrap();
        let keys: ArtKeys = parsed
            .data
            .into_values()
            .filter_map(|e| Some((e.name?, e.id?)))
            .collect();

        assert_eq!(keys.get("Wukong").map(String::as_str), Some("MonkeyKing"));
        assert_eq!(keys.get("Kai'Sa").map(String::as_str), Some("Kaisa"));
        assert_eq!(keys.get("Ahri").map(String::as_str), Some("Ahri"));
        // The key is never a lookup key itself — a card holds display names.
        assert_eq!(keys.get("MonkeyKing"), None);
    }

    /// Every field is optional because this is a remote document that can
    /// change without us. An entry missing either half is dropped rather
    /// than becoming a broken image URL.
    #[test]
    fn entries_missing_a_name_or_a_key_are_dropped() {
        let parsed: ChampionData = serde_json::from_str(
            r#"{"data":{
                "Ahri":{"id":"Ahri","name":"Ahri"},
                "NoName":{"id":"NoName"},
                "NoKey":{"name":"No Key"},
                "Empty":{}
            }}"#,
        )
        .unwrap();
        let keys: ArtKeys = parsed
            .data
            .into_values()
            .filter_map(|e| Some((e.name?, e.id?)))
            .collect();
        assert_eq!(keys.len(), 1);
    }

    #[test]
    fn a_document_with_no_data_object_is_empty_not_an_error() {
        let parsed: ChampionData = serde_json::from_str("{}").unwrap();
        assert!(parsed.data.is_empty());
    }
}
