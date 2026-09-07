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

use serde::{Deserialize, Serialize};

use crate::lcu::{LcuHttpClient, LockfileInfo};
use crate::warn;

const CDN: &str = "https://ddragon.leagueoflegends.com";

/// Community Dragon, which mirrors the game's *current* art rather than
/// Data Dragon's frozen set. Only summoner spells come from here, and only
/// when the League client is not running to serve them itself.
const COMMUNITY_CDN: &str =
    "https://raw.communitydragon.org/latest/plugins/rcp-be-lol-game-data/global/default";

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

/// Spell display name → its numeric id. The live client reports `Flash`
/// and match history reports `4`; the art is fetched by id either way, so
/// this is only ever used to get from one to the other.
type SpellNameIds = HashMap<String, i64>;

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
static SPELL_NAME_IDS: Mutex<Cached<SpellNameIds>> = Mutex::new(None);

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

/// Spell display name → numeric id, out of `summoner.json`. Data Dragon's
/// *data* is current even where its spell art is not, so this stays the
/// map even though the art comes from elsewhere.
async fn spell_name_ids(dir: &Path, version: &str) -> Option<Arc<SpellNameIds>> {
    if let Some(ids) = cached_map(&SPELL_NAME_IDS, version) {
        return Some(ids);
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

    let ids = Arc::new(name_to_id(parsed.data));
    *SPELL_NAME_IDS.lock().unwrap() = Some((version.to_string(), Arc::clone(&ids)));
    Some(ids)
}

/// Data Dragon writes the numeric id as a string. An entry whose `key` is
/// not a number is dropped rather than guessed at.
fn name_to_id(entries: HashMap<String, ChampionEntry>) -> SpellNameIds {
    entries
        .into_values()
        .filter_map(|entry| Some((entry.name?, entry.key?.parse().ok()?)))
        .collect()
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
///
/// Resolves the name to the numeric id and hands over, so a live-captured
/// scoreboard and a rebuilt one draw the same art from the same place.
/// `summoner.json` is still what maps between them — Data Dragon's *data*
/// is current even where its spell art is not.
pub async fn spell_icon(dir: &Path, lockfile: Option<&LockfileInfo>, spell: &str) -> Option<PathBuf> {
    let version = version(dir).await?;
    let id = *spell_name_ids(dir, &version).await?.get(spell)?;
    spell_icon_by_id(dir, lockfile, id).await
}

/// The cached icon for a summoner spell's numeric id.
///
/// **Not from Data Dragon.** Its `img/spell/` set is the pre-refresh art
/// and has been for years, so a Flash drawn from it does not match the one
/// in the game. There is no newer path on that CDN, so the art comes from
/// somewhere else entirely:
///
/// 1. **The running client's own asset store**, which is by definition the
///    art the game is using. It costs nothing new — the same host, the same
///    credentials, already reached for champion names.
/// 2. **Community Dragon**, when the client is not running, which is most
///    of the time a library is browsed. It mirrors the same game data from
///    a public CDN.
///
/// Whichever answers first is cached on disk, so one session with League
/// open is enough to fix every spell permanently.
pub async fn spell_icon_by_id(
    dir: &Path,
    lockfile: Option<&LockfileInfo>,
    spell_id: i64,
) -> Option<PathBuf> {
    if spell_id <= 0 {
        return None;
    }
    let version = version(dir).await?;
    let name = format!("{spell_id}.png");
    let path = dir.join(&version).join("spell").join(&name);
    if path.is_file() {
        return Some(path);
    }

    if let Some(bytes) = spell_from_client(lockfile, spell_id).await {
        return write_asset(&path, &bytes);
    }
    cached_file(
        dir,
        &version,
        "spell",
        &name,
        format!("{COMMUNITY_CDN}/v1/summoner-spells/{spell_id}.png"),
    )
    .await
}

/// The client's own icon for a spell, if a client is running.
async fn spell_from_client(lockfile: Option<&LockfileInfo>, spell_id: i64) -> Option<Vec<u8>> {
    let client = LcuHttpClient::new(lockfile?).ok()?;
    client
        .get_bytes(&format!("/lol-game-data/assets/v1/summoner-spells/{spell_id}.png"))
        .await
        .ok()
        .filter(|bytes| !bytes.is_empty())
}

fn write_asset(path: &Path, bytes: &[u8]) -> Option<PathBuf> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok()?;
    }
    std::fs::write(path, bytes).ok()?;
    Some(path.to_path_buf())
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

/// Resolves a page's worth of art, fetching whatever is not cached yet.
///
/// Sequential rather than parallel on purpose. The first call of a session
/// warms the version, three JSON documents and every icon at once; firing
/// that at a CDN as a hundred simultaneous requests is how an application
/// gets rate-limited, and the row renders without art in the meantime
/// either way.
pub async fn resolve_icons(dir: &Path, request: &IconRequest) -> IconSet {
    let mut set = IconSet::default();

    // Once per call, not once per spell: it is a file read, and whether a
    // client is running does not change halfway through a page.
    let lockfile = crate::lcu::lockfile::discover().ok().flatten();
    let lockfile = lockfile.as_ref();

    for champion in dedup(&request.champions) {
        if let Some(path) = champion_icon(dir, &champion).await {
            set.champions.insert(champion, display(path));
        }
    }
    for item in dedup(&request.items) {
        if let Some(path) = item_icon(dir, item).await {
            set.items.insert(item.to_string(), display(path));
        }
    }
    for spell in dedup(&request.spells) {
        if let Some(path) = spell_icon(dir, lockfile, &spell).await {
            set.spells.insert(spell, display(path));
        }
    }
    for spell in dedup(&request.spell_ids) {
        if let Some(path) = spell_icon_by_id(dir, lockfile, spell).await {
            set.spell_ids.insert(spell.to_string(), display(path));
        }
    }
    for rune in dedup(&request.runes) {
        if let Some(path) = rune_icon(dir, rune).await {
            set.runes.insert(rune.to_string(), display(path));
        }
    }
    set
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

    /// A live-captured scoreboard has `Flash`; a rebuilt one has `4`. Both
    /// have to end up fetching the same picture, so the name resolves to
    /// the id and the id is what the art is keyed on.
    #[test]
    fn spell_names_resolve_to_the_id_the_art_is_fetched_by() {
        let parsed: SpellData = serde_json::from_str(
            r#"{"data":{
                "SummonerFlash":{"id":"SummonerFlash","key":"4","name":"Flash"},
                "SummonerSmite":{"id":"SummonerSmite","key":"11","name":"Smite"}
            }}"#,
        )
        .unwrap();
        let ids = name_to_id(parsed.data);
        assert_eq!(ids.get("Flash"), Some(&4));
        assert_eq!(ids.get("Smite"), Some(&11));
    }

    /// Data Dragon writes the id as a string. Anything that is not a
    /// number is dropped rather than guessed at.
    #[test]
    fn a_spell_with_an_unparseable_key_is_dropped() {
        let parsed: SpellData = serde_json::from_str(
            r#"{"data":{
                "Good":{"id":"Good","key":"21","name":"Barrier"},
                "Bad":{"id":"Bad","key":"not a number","name":"Nonsense"},
                "None":{"id":"None","name":"Keyless"}
            }}"#,
        )
        .unwrap();
        let ids = name_to_id(parsed.data);
        assert_eq!(ids.len(), 1);
        assert_eq!(ids.get("Barrier"), Some(&21));
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
