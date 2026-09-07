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

/// Spell id → the asset's path **as the client writes it**, e.g.
/// `/lol-game-data/assets/DATA/Spells/Icons2D/Summoner_flash.png`. Kept in
/// that form because it is both a live LCU route and, once rewritten, a
/// Community Dragon one — `community_url` does the rewriting.
type SpellAssets = HashMap<i64, String>;

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
static SPELL_ASSETS: Mutex<Cached<SpellAssets>> = Mutex::new(None);

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

/// One entry of Community Dragon's `summoner-spells.json`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CommunitySpell {
    id: i64,
    icon_path: String,
}

/// Spell id → Community Dragon asset path.
///
/// There is no `summoner-spells/{id}.png` on that CDN — the first version
/// of this assumed there was and every spell silently resolved to nothing.
/// What it publishes is the client's own asset *manifest*, and each entry
/// carries the path the art really lives at.
async fn spell_assets(dir: &Path, version: &str) -> Option<Arc<SpellAssets>> {
    if let Some(assets) = cached_map(&SPELL_ASSETS, version) {
        return Some(assets);
    }
    // Cached under the Data Dragon version even though the document is
    // unversioned: a patch is exactly when the art can change, and it
    // keeps the cache one directory per version with nothing outside it.
    let body = cached_json(
        dir,
        version,
        "summoner-spells.json",
        format!("{COMMUNITY_CDN}/v1/summoner-spells.json"),
    )
    .await?;

    let parsed: Vec<CommunitySpell> = serde_json::from_str(&body)
        .map_err(|e| warn!("ddragon", "summoner-spells.json did not parse: {e}"))
        .ok()?;

    let assets = Arc::new(spell_asset_map(parsed));
    *SPELL_ASSETS.lock().unwrap() = Some((version.to_string(), Arc::clone(&assets)));
    Some(assets)
}

fn spell_asset_map(entries: Vec<CommunitySpell>) -> SpellAssets {
    entries
        .into_iter()
        .filter(|entry| entry.icon_path.starts_with(CLIENT_ASSET_ROOT))
        .map(|entry| (entry.id, entry.icon_path))
        .collect()
}

/// The prefix the client puts in front of every asset path it publishes.
/// It is a route on the client's own HTTP server, and the part after it is
/// what Community Dragon mirrors.
const CLIENT_ASSET_ROOT: &str = "/lol-game-data/assets";

/// Rewrites a client asset path into the Community Dragon URL for it.
///
/// The rule is the CDN's own: drop the `/lol-game-data/assets` prefix and
/// lowercase the rest. `Summoner_flash.png` under `DATA/Spells/Icons2D`
/// becomes `/data/spells/icons2d/summoner_flash.png`, and the same rule
/// carries the odd ones — `Summoner_Teleport_New.png`, which has mixed
/// case in the filename and not just the directories, and the Jade spells
/// filed under `ASSETS/UX` rather than `DATA/Spells`.
fn community_url(icon_path: &str) -> Option<String> {
    let path = icon_path.strip_prefix(CLIENT_ASSET_ROOT)?;
    Some(format!("{COMMUNITY_CDN}{}", path.to_lowercase()))
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

    // Client art and fallback art are cached under *different* names, and
    // the client's is checked first. Sharing one name made the fallback
    // permanent: whichever source answered on the cold cache won forever,
    // so a library first opened with League closed would never take the
    // client's art no matter how many sessions ran with it open — which is
    // the opposite of what this function is documented to do.
    let from_client = dir.join(&version).join("spell").join(format!("{spell_id}.client.png"));
    if from_client.is_file() {
        return Some(from_client);
    }

    let icon_path = spell_assets(dir, &version).await?.get(&spell_id)?.clone();

    if lockfile.is_some() {
        if let Some(bytes) = spell_from_client(lockfile, &icon_path).await {
            return write_asset(&from_client, &bytes);
        }
    }
    cached_file(
        dir,
        &version,
        "spell",
        &format!("{spell_id}.png"),
        community_url(&icon_path)?,
    )
    .await
}

/// The client's own icon for a spell, if a client is running.
///
/// `icon_path` is already a route on the client's HTTP server — that is
/// what the manifest publishes — so it is asked for verbatim. The first
/// version of this built `v1/summoner-spells/{id}.png` by analogy instead,
/// which no client has ever served.
async fn spell_from_client(lockfile: Option<&LockfileInfo>, icon_path: &str) -> Option<Vec<u8>> {
    let client = LcuHttpClient::new(lockfile?).ok()?;
    client
        .get_bytes(icon_path)
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
    // Once per call, not once per spell: it is a file read, and whether a
    // client is running does not change halfway through a page.
    let lockfile = crate::lcu::lockfile::discover().ok().flatten();
    let lockfile = lockfile.as_ref();

    // The shared documents are warmed first, in series. Six tasks each
    // finding an empty map would each fetch `champion.json` — the
    // duplicate-request problem the cache exists to avoid, multiplied by
    // the concurrency.
    if let Some(version) = version(dir).await {
        if !request.champions.is_empty() {
            let _ = art_keys(dir, &version).await;
        }
        if !request.spells.is_empty() {
            let _ = spell_name_ids(dir, &version).await;
        }
        if !request.spells.is_empty() || !request.spell_ids.is_empty() {
            let _ = spell_assets(dir, &version).await;
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
            spell_icon(dir, lockfile, &name).await
        })
        .await,
        spell_ids: resolve_each(dedup(&request.spell_ids), |id: i64| async move {
            spell_icon_by_id(dir, lockfile, id).await
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

    /// The rewrite is the whole fallback. The first attempt guessed at a
    /// `summoner-spells/{id}.png` route that does not exist, which fails
    /// *silently* — a 404 is indistinguishable from "no art" downstream —
    /// so the real document's shape is pinned here.
    #[test]
    fn client_asset_paths_become_community_dragon_ones() {
        let parsed: Vec<CommunitySpell> = serde_json::from_str(
            r#"[
                {"id":4,"name":"Flash","iconPath":"/lol-game-data/assets/DATA/Spells/Icons2D/Summoner_flash.png"},
                {"id":12,"name":"Teleport","iconPath":"/lol-game-data/assets/DATA/Spells/Icons2D/Summoner_Teleport_New.png"},
                {"id":74,"name":"Flash","iconPath":"/lol-game-data/assets/ASSETS/UX/Jade/S3Icons/SameSized/S3_Summoner_flash.project_jade.png"}
            ]"#,
        )
        .unwrap();
        let assets = spell_asset_map(parsed);

        // The map keeps the client's own path, because that is also a live
        // route on the client's HTTP server.
        assert_eq!(
            assets.get(&4).map(String::as_str),
            Some("/lol-game-data/assets/DATA/Spells/Icons2D/Summoner_flash.png")
        );

        assert_eq!(
            community_url(&assets[&4]).unwrap(),
            format!("{COMMUNITY_CDN}/data/spells/icons2d/summoner_flash.png")
        );
        // Mixed case in the filename is lowercased too, not just the dirs.
        assert_eq!(
            community_url(&assets[&12]).unwrap(),
            format!("{COMMUNITY_CDN}/data/spells/icons2d/summoner_teleport_new.png")
        );
        // Not every spell lives under DATA/Spells — the Jade set does not.
        assert_eq!(
            community_url(&assets[&74]).unwrap(),
            format!("{COMMUNITY_CDN}/assets/ux/jade/s3icons/samesized/s3_summoner_flash.project_jade.png")
        );
    }

    /// An entry whose path is not under the prefix the rule strips is
    /// dropped rather than turned into a URL that 404s.
    #[test]
    fn spell_entries_outside_the_asset_root_are_dropped() {
        let parsed: Vec<CommunitySpell> = serde_json::from_str(
            r#"[{"id":9001,"iconPath":"https://example.invalid/elsewhere.png"}]"#,
        )
        .unwrap();
        assert!(spell_asset_map(parsed).is_empty());
        assert_eq!(community_url("https://example.invalid/elsewhere.png"), None);
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
