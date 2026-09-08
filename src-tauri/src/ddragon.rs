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
//! by zero and the disk cost is what the user actually met — a champion
//! square is about 7 KB. The library row draws both team compositions, so
//! that is the champions someone was *in a game with* rather than the ones
//! they played, and it converges on most of the roster: about 170 squares,
//! near enough 1.2 MB, bounded by the game rather than by how many
//! recordings there are. See DEVELOPMENT.md §5.3.
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

use serde::{Deserialize, Serialize};

use crate::warn;

/// The only place art comes from. Spells briefly took a detour through the
/// running client and Community Dragon, on the theory that Data Dragon's
/// `img/spell/` set was too old to match the game; the art was never the
/// problem (see `spell_art_map`), and three sources meant three ways for a
/// row to draw the wrong picture.
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
    /// The numeric id, as a string — Data Dragon's own spelling. Only the
    /// spell map reads it: a scoreboard rebuilt from match history has
    /// spell *ids* where a live one has names.
    #[serde(default)]
    key: Option<String>,
}

/// Display name → the key its art is filed under. Champions and summoner
/// spells have the same problem and the same shape of answer.
type ArtKeys = HashMap<String, String>;

/// Rune (or rune tree) id → the icon path Data Dragon serves it under.
type RuneIcons = HashMap<i64, String>;

/// Both ways into summoner spell art, built from one parse of
/// `summoner.json`. A live-captured scoreboard has the display name
/// (`Flash`), one rebuilt from match history has the numeric id (`4`), and
/// both have to land on the same picture.
#[derive(Debug)]
struct SpellArt {
    /// Display name → the key art is filed under (`SummonerFlash`).
    by_name: HashMap<String, String>,
    /// Numeric id → the same key. Keyed on *every* variant's id, so 74 and
    /// 2202 resolve to `SummonerFlash` alongside 4.
    by_id: HashMap<i64, String>,
}

/// Data Dragon's `summoner.json`, reduced the same way `champion.json` is.
#[derive(Debug, Deserialize)]
struct SpellData {
    #[serde(default)]
    data: HashMap<String, ChampionEntry>,
}

/// One tree from `runesReforged.json`. The tree itself has an icon, and so
/// does every rune in every slot — all of them keyed by id, all of them a
/// path rather than a filename.
#[derive(Debug, Deserialize)]
struct RuneTree {
    #[serde(default)]
    id: Option<i64>,
    #[serde(default)]
    icon: Option<String>,
    #[serde(default)]
    slots: Vec<RuneSlot>,
}

#[derive(Debug, Deserialize)]
struct RuneSlot {
    #[serde(default)]
    runes: Vec<RuneEntry>,
}

#[derive(Debug, Deserialize)]
struct RuneEntry {
    #[serde(default)]
    id: Option<i64>,
    #[serde(default)]
    icon: Option<String>,
}

/// Flattens the tree-of-slots-of-runes into id → icon path, trees
/// included: a row shows the keystone and both tree crests, and all three
/// are looked up the same way.
fn rune_icon_map(trees: Vec<RuneTree>) -> RuneIcons {
    let mut icons = RuneIcons::new();
    for tree in trees {
        if let (Some(id), Some(icon)) = (tree.id, tree.icon.clone()) {
            icons.insert(id, icon);
        }
        for slot in tree.slots {
            for rune in slot.runes {
                if let (Some(id), Some(icon)) = (rune.id, rune.icon) {
                    icons.insert(id, icon);
                }
            }
        }
    }
    icons
}

type Cached<T> = Option<(String, Arc<T>)>;

/// Resolved once per process, per document. Two callers racing costs a
/// duplicate request for a static file, which is a better trade than
/// holding a lock across an await.
static CHAMPION_KEYS: Mutex<Cached<ArtKeys>> = Mutex::new(None);
static RUNE_ICONS: Mutex<Cached<RuneIcons>> = Mutex::new(None);
static SPELL_ART: Mutex<Cached<SpellArt>> = Mutex::new(None);

/// Reads a cached map if it belongs to `version`.
fn cached_map<T>(cache: &Mutex<Cached<T>>, version: &str) -> Option<Arc<T>> {
    let guard = cache.lock().unwrap();
    match guard.as_ref() {
        Some((cached, map)) if cached == version => Some(Arc::clone(map)),
        _ => None,
    }
}

/// Fetches a Data Dragon JSON document, caching the raw body on disk so a
/// later session parses it without a request.
async fn cached_json(dir: &Path, version: &str, name: &str, url: String) -> Option<String> {
    let path = dir.join(version).join(name);
    if let Ok(body) = std::fs::read_to_string(&path) {
        return Some(body);
    }
    let body = client()?
        .get(url)
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
    Some(body)
}

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
    if let Some(keys) = cached_map(&CHAMPION_KEYS, version) {
        return Some(keys);
    }
    let body = cached_json(
        dir,
        version,
        "champion.json",
        format!("{CDN}/cdn/{version}/data/en_US/champion.json"),
    )
    .await?;

    let parsed: ChampionData = serde_json::from_str(&body)
        .map_err(|e| warn!("ddragon", "champion.json did not parse: {e}"))
        .ok()?;

    let keys = Arc::new(name_to_key(parsed.data));
    *CHAMPION_KEYS.lock().unwrap() = Some((version.to_string(), Arc::clone(&keys)));
    Some(keys)
}

/// Spell art keys, out of `summoner.json`.
async fn spell_art(dir: &Path, version: &str) -> Option<Arc<SpellArt>> {
    if let Some(art) = cached_map(&SPELL_ART, version) {
        return Some(art);
    }
    let body = cached_json(
        dir,
        version,
        "summoner.json",
        format!("{CDN}/cdn/{version}/data/en_US/summoner.json"),
    )
    .await?;

    let parsed: SpellData = serde_json::from_str(&body)
        .map_err(|e| warn!("ddragon", "summoner.json did not parse: {e}"))
        .ok()?;

    let art = Arc::new(spell_art_map(parsed.data));
    *SPELL_ART.lock().unwrap() = Some((version.to_string(), Arc::clone(&art)));
    Some(art)
}

/// Picks one art key per spell, collapsing every game-mode variant onto
/// the standard version.
///
/// **`summoner.json` has one entry per variant, not per spell.** `Flash` is
/// three of them — `SummonerFlash` (4), `SummonerFlash_Jade` (74) and
/// `SummonerCherryFlash` (2202) — and nine other names collide the same
/// way. Collecting them straight into a map keyed by name therefore let
/// whichever entry `HashMap` iteration happened to reach last win, so the
/// Flash on a row was the Arena set's armoured figure on one run and the
/// right picture on the next. That non-determinism, not stale Data Dragon
/// art, is what made spells look broken.
///
/// The pick is deliberate instead: prefer an art key with **no underscore**
/// — which is what separates `SummonerFlash` from `SummonerFlash_Jade` —
/// and then the **lowest id**, which separates it from
/// `SummonerCherryFlash`. A preference and not a filter, so a name that
/// exists *only* as a variant (`Fortify`, `Revive`, the rest of the retired
/// set) still resolves to something rather than to nothing.
fn spell_art_map(entries: HashMap<String, ChampionEntry>) -> SpellArt {
    // (id, art key, display name). An entry missing any of the three is
    // dropped rather than guessed at — Data Dragon writes the id as a
    // string, so a `key` that is not a number is as unusable as a missing
    // one.
    let mut spells: Vec<(i64, String, String)> = entries
        .into_values()
        .filter_map(|entry| Some((entry.key?.parse().ok()?, entry.id?, entry.name?)))
        .collect();

    // Best first, so the first entry seen for a name is the one to keep.
    spells.sort_by(|a, b| is_variant(&a.1).cmp(&is_variant(&b.1)).then(a.0.cmp(&b.0)));

    let mut by_name: HashMap<String, String> = HashMap::new();
    for (_, art_key, name) in &spells {
        by_name.entry(name.clone()).or_insert_with(|| art_key.clone());
    }
    // Every variant id points at the name's chosen key, not at its own, so
    // a match-history scoreboard carrying 74 draws the standard Flash.
    let by_id = spells
        .iter()
        .filter_map(|(id, _, name)| Some((*id, by_name.get(name)?.clone())))
        .collect();

    SpellArt { by_name, by_id }
}

/// Whether an art key names an alternate-mode version of a spell rather
/// than the spell. Riot files every one of those sets with a suffix —
/// `SummonerFlash_Jade`, `Summoner_UltBookPlaceholder` — so the underscore
/// is the whole test.
fn is_variant(art_key: &str) -> bool {
    art_key.contains('_')
}

/// The art key for a spell display name, falling back to the name's last
/// word when the game has upgraded the spell.
///
/// **The game renames a spell in place when it is upgraded, and Data Dragon
/// has an entry for none of the upgraded names.** A real ranked capture
/// carried two of them at once: `Primal Smite` on both junglers, and
/// `Unleashed Teleport` on four other players. Every one of those drew an
/// empty circle.
///
/// Each is the base spell with a word in front of it, so the last word is
/// the base name — `Smite`, `Teleport` — and the base art is what the game
/// draws in both states anyway. Doing it this way rather than listing the
/// upgrade names means the next rename costs nothing: Riot has already
/// shipped `Chilling Smite` and `Challenging Smite` under this scheme.
///
/// The full name is tried **first**, so the multi-word names that are
/// spells in their own right (`Poro Toss`, `To the King!`) are untouched,
/// and a last word that resolves to nothing still yields `None` rather than
/// the wrong picture.
fn art_key_for(by_name: &HashMap<String, String>, spell: &str) -> Option<String> {
    let spell = spell.trim();
    if let Some(key) = by_name.get(spell) {
        return Some(key.clone());
    }
    let (_, base) = spell.rsplit_once(' ')?;
    by_name.get(base).cloned()
}

/// Rune id → icon path, flattened out of the tree document.
async fn rune_icons(dir: &Path, version: &str) -> Option<Arc<RuneIcons>> {
    if let Some(icons) = cached_map(&RUNE_ICONS, version) {
        return Some(icons);
    }
    let body = cached_json(
        dir,
        version,
        "runesReforged.json",
        format!("{CDN}/cdn/{version}/data/en_US/runesReforged.json"),
    )
    .await?;

    let trees: Vec<RuneTree> = serde_json::from_str(&body)
        .map_err(|e| warn!("ddragon", "runesReforged.json did not parse: {e}"))
        .ok()?;

    let icons = Arc::new(rune_icon_map(trees));
    *RUNE_ICONS.lock().unwrap() = Some((version.to_string(), Arc::clone(&icons)));
    Some(icons)
}

/// Both `champion.json` and `summoner.json` are an object of entries with
/// a display `name` and the `id` art is filed under. An entry missing
/// either half is dropped rather than becoming a broken image URL.
fn name_to_key(entries: HashMap<String, ChampionEntry>) -> ArtKeys {
    entries
        .into_values()
        .filter_map(|entry| Some((entry.name?, entry.id?)))
        .collect()
}

/// Fetches one asset into the cache if it is not there, and hands back the
/// path either way.
///
/// `sub` is the directory under the version — `champion`, `item`, `spell`,
/// `rune` — and `name` the file inside it. `url` is asked for only on a
/// miss, so a warm cache makes no request at all.
async fn cached_file(dir: &Path, version: &str, sub: &str, name: &str, url: String) -> Option<PathBuf> {
    let path = dir.join(version).join(sub).join(name);
    if path.is_file() {
        return Some(path);
    }

    let bytes = client()?
        .get(url)
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

/// The cached square portrait for a champion display name.
///
/// `None` for everything that could go wrong — no network, an unknown
/// champion, an unwritable cache — because the caller's fallback is the
/// text that was on the row before any of this existed. That holds for
/// every resolver below too.
pub async fn champion_icon(dir: &Path, champion: &str) -> Option<PathBuf> {
    let version = version(dir).await?;
    let key = art_keys(dir, &version).await?.get(champion)?.clone();
    let url = format!("{CDN}/cdn/{version}/img/champion/{key}.png");
    cached_file(dir, &version, "champion", &format!("{key}.png"), url).await
}

/// The cached icon for an item id.
///
/// The only one of these that needs no map: Data Dragon files item art
/// under the numeric id the game itself reports.
pub async fn item_icon(dir: &Path, item_id: i64) -> Option<PathBuf> {
    if item_id <= 0 {
        return None;
    }
    let version = version(dir).await?;
    let url = format!("{CDN}/cdn/{version}/img/item/{item_id}.png");
    cached_file(dir, &version, "item", &format!("{item_id}.png"), url).await
}

/// The cached icon for a summoner spell's display name.
pub async fn spell_icon(dir: &Path, spell: &str) -> Option<PathBuf> {
    let version = version(dir).await?;
    let key = art_key_for(&spell_art(dir, &version).await?.by_name, spell)?;
    spell_art_file(dir, &version, &key).await
}

/// The cached icon for a summoner spell's numeric id, which is what a
/// scoreboard rebuilt from match history carries.
pub async fn spell_icon_by_id(dir: &Path, spell_id: i64) -> Option<PathBuf> {
    if spell_id <= 0 {
        return None;
    }
    let version = version(dir).await?;
    let key = spell_art(dir, &version).await?.by_id.get(&spell_id)?.clone();
    spell_art_file(dir, &version, &key).await
}

/// Both paths above end here, so a name and an id that mean the same spell
/// share one cache file instead of two copies under different names.
async fn spell_art_file(dir: &Path, version: &str, art_key: &str) -> Option<PathBuf> {
    let url = format!("{CDN}/cdn/{version}/img/spell/{art_key}.png");
    cached_file(dir, version, "spell", &format!("{art_key}.png"), url).await
}

/// The cached icon for a rune or rune tree id.
///
/// Runes are the odd one out twice over: the icon is a *path* rather than
/// a filename, and it is served from an **unversioned** part of the CDN.
/// The path is flattened into a single cache filename so the layout on
/// disk stays one directory per kind.
pub async fn rune_icon(dir: &Path, rune_id: i64) -> Option<PathBuf> {
    if rune_id <= 0 {
        return None;
    }
    let version = version(dir).await?;
    let icon = rune_icons(dir, &version).await?.get(&rune_id)?.clone();
    let name = format!("{rune_id}.png");
    let url = format!("{CDN}/cdn/img/{icon}");
    cached_file(dir, &version, "rune", &name, url).await
}

/// What a page of rows needs drawing, asked for in one go.
///
/// A row carries up to seven items, two spells, three runes and a
/// champion, and a library shows dozens of rows — one IPC call per icon
/// would be hundreds. The frontend collects everything visible, asks once,
/// and keys what comes back.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IconRequest {
    #[serde(default)]
    pub champions: Vec<String>,
    #[serde(default)]
    pub items: Vec<i64>,
    #[serde(default)]
    pub spells: Vec<String>,
    /// The same spells as ids, for a scoreboard rebuilt from match
    /// history — it reports ids where the live client reports names.
    #[serde(default)]
    pub spell_ids: Vec<i64>,
    #[serde(default)]
    pub runes: Vec<i64>,
}

/// Paths for everything that resolved. **Anything that did not is simply
/// absent** rather than present-and-null: the caller's fallback is the
/// text that was on the row before any of this existed, and a missing key
/// says that more plainly than a null does.
#[derive(Debug, Default, Serialize)]
pub struct IconSet {
    pub champions: HashMap<String, String>,
    pub items: HashMap<String, String>,
    pub spells: HashMap<String, String>,
    pub spell_ids: HashMap<String, String>,
    pub runes: HashMap<String, String>,
}

/// How many icons to have in flight at once.
///
/// Six, which is what a browser allows per host, and the same reasoning:
/// enough that a page of art arrives in a couple of rounds rather than a
/// couple of dozen, few enough that a cold cache does not reach a CDN as a
/// burst. Sequential was the first answer and it was the wrong end of that
/// trade — fourteen distinct icons meant fourteen round trips in series,
/// which is long enough to sit and watch.
const CONCURRENCY: usize = 6;

/// Resolves a page's worth of art, fetching whatever is not cached yet.
pub async fn resolve_icons(dir: &Path, request: &IconRequest) -> IconSet {
    // The shared documents are warmed first, in series. Six tasks each
    // finding an empty map would each fetch `champion.json` — the
    // duplicate-request problem the cache exists to avoid, multiplied by
    // the concurrency.
    if let Some(version) = version(dir).await {
        if !request.champions.is_empty() {
            let _ = art_keys(dir, &version).await;
        }
        if !request.spells.is_empty() || !request.spell_ids.is_empty() {
            let _ = spell_art(dir, &version).await;
        }
        if !request.runes.is_empty() {
            let _ = rune_icons(dir, &version).await;
        }
    }

    IconSet {
        champions: resolve_each(dedup(&request.champions), |name: String| async move {
            champion_icon(dir, &name).await
        })
        .await,
        items: resolve_each(dedup(&request.items), |id: i64| async move {
            item_icon(dir, id).await
        })
        .await,
        spells: resolve_each(dedup(&request.spells), |name: String| async move {
            spell_icon(dir, &name).await
        })
        .await,
        spell_ids: resolve_each(dedup(&request.spell_ids), |id: i64| async move {
            spell_icon_by_id(dir, id).await
        })
        .await,
        runes: resolve_each(dedup(&request.runes), |id: i64| async move {
            rune_icon(dir, id).await
        })
        .await,
    }
}

/// Runs `fetch` over `keys` with at most `CONCURRENCY` in flight, keeping
/// only the ones that resolved.
///
/// Unordered, because nothing downstream cares: the result is a map and the
/// frontend paints whatever is in it.
async fn resolve_each<K, F, Fut>(keys: Vec<K>, fetch: F) -> HashMap<String, String>
where
    K: std::fmt::Display,
    F: Fn(K) -> Fut,
    Fut: std::future::Future<Output = Option<PathBuf>>,
{
    use futures_util::stream::StreamExt;

    futures_util::stream::iter(keys)
        .map(|key| {
            let fetch = &fetch;
            async move { (key.to_string(), fetch(key).await) }
        })
        .buffer_unordered(CONCURRENCY)
        .filter_map(|(key, path)| async move { path.map(|p| (key, display(p))) })
        .collect()
        .await
}

/// Ten rows sharing a champion should cost one lookup, not ten.
fn dedup<T: Clone + Eq + std::hash::Hash>(values: &[T]) -> Vec<T> {
    let mut seen = std::collections::HashSet::new();
    values.iter().filter(|v| seen.insert((*v).clone())).cloned().collect()
}

fn display(path: PathBuf) -> String {
    path.to_string_lossy().into_owned()
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

    /// Runes are the awkward one: the document is trees of slots of runes,
    /// the icon is a path rather than a filename, and a row needs both the
    /// keystone and the tree crests — so trees and runes flatten into one
    /// map keyed the same way.
    #[test]
    fn rune_trees_flatten_into_one_map_of_ids() {
        let trees: Vec<RuneTree> = serde_json::from_str(
            r#"[{
                "id": 8100,
                "icon": "perk-images/Styles/7200_Domination.png",
                "slots": [
                    {"runes": [
                        {"id": 8112, "icon": "perk-images/Styles/Domination/Electrocute/Electrocute.png"},
                        {"id": 8124, "icon": "perk-images/Styles/Domination/Predator/Predator.png"}
                    ]},
                    {"runes": [{"id": 8126, "icon": "perk-images/Styles/Domination/CheapShot/CheapShot.png"}]}
                ]
            }]"#,
        )
        .unwrap();
        let icons = rune_icon_map(trees);

        // The tree crest and every rune under it, from every slot.
        assert_eq!(icons.len(), 4);
        assert_eq!(
            icons.get(&8112).map(String::as_str),
            Some("perk-images/Styles/Domination/Electrocute/Electrocute.png")
        );
        assert_eq!(
            icons.get(&8100).map(String::as_str),
            Some("perk-images/Styles/7200_Domination.png")
        );
    }

    /// Same rule as everywhere else here: a remote document that changed
    /// under us costs the icons it describes and nothing more.
    #[test]
    fn runes_missing_an_id_or_an_icon_are_dropped() {
        let trees: Vec<RuneTree> = serde_json::from_str(
            r#"[{"slots": [{"runes": [
                {"id": 1, "icon": "a.png"},
                {"id": 2},
                {"icon": "c.png"},
                {}
            ]}]}]"#,
        )
        .unwrap();
        assert_eq!(rune_icon_map(trees).len(), 1);
    }

    /// `summoner.json`'s `data` object, which is all these tests vary.
    fn spells(data: &str) -> HashMap<String, ChampionEntry> {
        serde_json::from_str::<SpellData>(&format!(r#"{{"data":{data}}}"#))
            .unwrap()
            .data
    }

    /// A live-captured scoreboard has `Flash`; a rebuilt one has `4`. Both
    /// have to end up drawing the same picture, so both maps carry the same
    /// art key.
    #[test]
    fn a_spell_resolves_to_the_same_art_by_name_and_by_id() {
        let art = spell_art_map(spells(
            r#"{
                "SummonerFlash":{"id":"SummonerFlash","key":"4","name":"Flash"},
                "SummonerSmite":{"id":"SummonerSmite","key":"11","name":"Smite"}
            }"#,
        ));
        assert_eq!(art.by_name.get("Flash").map(String::as_str), Some("SummonerFlash"));
        assert_eq!(art.by_id.get(&4).map(String::as_str), Some("SummonerFlash"));
        assert_eq!(art.by_name.get("Smite").map(String::as_str), Some("SummonerSmite"));
        assert_eq!(art.by_id.get(&11).map(String::as_str), Some("SummonerSmite"));
    }

    /// The bug this map exists for. `Flash` names three entries in the real
    /// document, and building the map by `collect` let `HashMap` iteration
    /// order decide which one a row drew — so the Arena set's armoured
    /// figure turned up at random. The standard spell has to win every
    /// time, by name *and* by any of the variants' ids.
    #[test]
    fn every_variant_of_a_spell_resolves_to_the_standard_art() {
        let art = spell_art_map(spells(
            r#"{
                "SummonerFlash_Jade":{"id":"SummonerFlash_Jade","key":"74","name":"Flash"},
                "SummonerCherryFlash":{"id":"SummonerCherryFlash","key":"2202","name":"Flash"},
                "SummonerFlash":{"id":"SummonerFlash","key":"4","name":"Flash"}
            }"#,
        ));
        assert_eq!(art.by_name.get("Flash").map(String::as_str), Some("SummonerFlash"));
        for id in [4, 74, 2202] {
            assert_eq!(
                art.by_id.get(&id).map(String::as_str),
                Some("SummonerFlash"),
                "id {id} drew the wrong Flash"
            );
        }
    }

    /// The underscore rule is a *preference*, not a filter. Several retired
    /// spells exist only as a Jade entry, and dropping them outright would
    /// resolve them to nothing instead of to the one picture there is.
    #[test]
    fn a_spell_that_only_exists_as_a_variant_still_resolves() {
        let art = spell_art_map(spells(
            r#"{"SummonerFortify_Jade":{"id":"SummonerFortify_Jade","key":"705","name":"Fortify"}}"#,
        ));
        assert_eq!(
            art.by_name.get("Fortify").map(String::as_str),
            Some("SummonerFortify_Jade")
        );
    }

    /// Data Dragon writes the id as a string. Anything that is not a
    /// number is dropped rather than guessed at, as is an entry missing a
    /// name or the key its art is filed under.
    #[test]
    fn a_spell_missing_a_usable_key_is_dropped() {
        let art = spell_art_map(spells(
            r#"{
                "Good":{"id":"SummonerBarrier","key":"21","name":"Barrier"},
                "Bad":{"id":"Bad","key":"not a number","name":"Nonsense"},
                "Keyless":{"id":"Keyless","name":"Keyless"},
                "Nameless":{"id":"Nameless","key":"99"}
            }"#,
        ));
        assert_eq!(art.by_name.len(), 1);
        assert_eq!(art.by_id.len(), 1);
        assert_eq!(art.by_name.get("Barrier").map(String::as_str), Some("SummonerBarrier"));
    }

    /// The names in this test are the ones a real ranked capture actually
    /// carried: both junglers on `Primal Smite`, four other players on
    /// `Unleashed Teleport`. Data Dragon has an entry for neither, so
    /// before the fallback every one of those slots drew an empty circle.
    #[test]
    fn an_upgraded_spell_falls_back_to_the_base_art() {
        let art = spell_art_map(spells(
            r#"{
                "SummonerSmite":{"id":"SummonerSmite","key":"11","name":"Smite"},
                "SummonerTeleport":{"id":"SummonerTeleport","key":"12","name":"Teleport"}
            }"#,
        ));
        for name in ["Primal Smite", "Unleashed Smite", "Chilling Smite", "Smite"] {
            assert_eq!(
                art_key_for(&art.by_name, name).as_deref(),
                Some("SummonerSmite"),
                "{name} did not fall back"
            );
        }
        for name in ["Unleashed Teleport", "Teleport", "  Unleashed Teleport  "] {
            assert_eq!(
                art_key_for(&art.by_name, name).as_deref(),
                Some("SummonerTeleport"),
                "{name} did not fall back"
            );
        }
    }

    /// The full name wins over the fallback, so a multi-word spell that is
    /// a spell in its own right is never reduced to its last word.
    #[test]
    fn a_multi_word_spell_is_not_reduced_to_its_last_word() {
        let art = spell_art_map(spells(
            r#"{
                "SummonerPoroThrow":{"id":"SummonerPoroThrow","key":"31","name":"Poro Toss"},
                "SummonerSnowball":{"id":"SummonerSnowball","key":"32","name":"Toss"}
            }"#,
        ));
        assert_eq!(
            art_key_for(&art.by_name, "Poro Toss").as_deref(),
            Some("SummonerPoroThrow")
        );
    }

    /// A name whose last word resolves to nothing stays unresolved. The
    /// row falls back to its text, which is the contract everywhere here —
    /// drawing *some* spell would be worse than drawing none.
    #[test]
    fn an_unknown_spell_stays_unknown() {
        let art = spell_art_map(spells(
            r#"{"SummonerFlash":{"id":"SummonerFlash","key":"4","name":"Flash"}}"#,
        ));
        assert_eq!(art_key_for(&art.by_name, "Arcane Doohickey"), None);
        assert_eq!(art_key_for(&art.by_name, ""), None);
        assert_eq!(art_key_for(&art.by_name, "Flash").as_deref(), Some("SummonerFlash"));
    }

    /// The concurrency bound is the whole point of `resolve_each`, and it
    /// is invisible in the output — a map resolved six at a time and one
    /// resolved a hundred at a time are the same map. So it is asserted
    /// directly: without this, raising `CONCURRENCY` to something that
    /// looks like a burst to a CDN would pass every other test here.
    #[tokio::test]
    async fn no_more_than_the_bound_are_ever_in_flight() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let live = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));

        let keys: Vec<i64> = (0..CONCURRENCY as i64 * 3).collect();
        let resolved = resolve_each(keys, |id: i64| {
            let (live, peak) = (live.clone(), peak.clone());
            async move {
                let now = live.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                live.fetch_sub(1, Ordering::SeqCst);
                Some(PathBuf::from(format!("{id}.png")))
            }
        })
        .await;

        assert_eq!(peak.load(Ordering::SeqCst), CONCURRENCY);
        assert_eq!(resolved.len(), CONCURRENCY * 3);
        assert!(live.load(Ordering::SeqCst) == 0, "a fetch outlived the stream");
    }

    /// A key that could not be resolved is **absent**, not present and
    /// empty. The frontend falls back to the text that was there before
    /// art existed by asking whether the key is in the map, so an empty
    /// string would paint a broken image over a perfectly good name.
    #[tokio::test]
    async fn unresolved_keys_stay_out_of_the_map() {
        let resolved = resolve_each(vec![1i64, 2, 3, 4], |id: i64| async move {
            (id % 2 == 0).then(|| PathBuf::from(format!("{id}.png")))
        })
        .await;

        assert_eq!(resolved.len(), 2);
        assert!(resolved.contains_key("2"));
        assert!(!resolved.contains_key("1"), "a miss leaked into the map");
    }

    /// Nothing asked for is nothing fetched — a library with no art to
    /// resolve must not reach the network to find that out.
    #[tokio::test]
    async fn an_empty_request_fetches_nothing() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let resolved = resolve_each(Vec::<i64>::new(), move |_: i64| {
            let c = c.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                None
            }
        })
        .await;

        assert!(resolved.is_empty());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    /// Ten rows sharing a champion must cost one lookup, not ten.
    #[test]
    fn duplicate_requests_are_asked_for_once() {
        assert_eq!(dedup(&["Ahri", "Wukong", "Ahri", "Ahri"]), vec!["Ahri", "Wukong"]);
        assert_eq!(dedup(&[3089i64, 3089, 3157]), vec![3089, 3157]);
    }

    #[test]
    fn a_document_with_no_data_object_is_empty_not_an_error() {
        let parsed: ChampionData = serde_json::from_str("{}").unwrap();
        assert!(parsed.data.is_empty());
    }
}
