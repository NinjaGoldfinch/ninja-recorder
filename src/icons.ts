/**
 * Data Dragon art, and Community Dragon's position icons, asked for once per
 * page rather than once per icon.
 *
 * A row can carry a champion portrait, two summoner spells, three runes
 * and seven items. A library of forty rows is therefore several hundred
 * icons, and one IPC call each — every one of them a CDN round trip the
 * first time — would be a library that renders over several seconds.
 *
 * So the row template renders empty slots, this collects everything those
 * slots want, asks once, and fills them in afterwards. The row is correct
 * before any of it arrives: every slot falls back to the text or the blank
 * that was there before art existed, which is also what an offline session
 * gets forever.
 */
import { assetUrl, call } from "./bridge";
import type { IconSet, RecordingRow, Scoreboard, ScoreboardRunes } from "./types";

/** Resolved paths, and the misses. A `null` is "asked and not found", which
 *  is different from "not asked yet" and stops a missing icon being
 *  re-requested on every render. */
const cache = {
  champions: new Map<string, string | null>(),
  items: new Map<number, string | null>(),
  spells: new Map<string, string | null>(),
  spellIds: new Map<number, string | null>(),
  runes: new Map<number, string | null>(),
  positions: new Map<string, string | null>(),
  profileIcons: new Map<number, string | null>(),
};

export function championIcon(name: string | null): string | null {
  return name === null ? null : (cache.champions.get(name) ?? null);
}

export function itemIcon(id: number): string | null {
  return cache.items.get(id) ?? null;
}

export function spellIcon(name: string): string | null {
  return cache.spells.get(name) ?? null;
}

/** By id, which is what a scoreboard rebuilt from match history carries. */
export function spellIconById(id: number): string | null {
  return cache.spellIds.get(id) ?? null;
}

export function runeIcon(id: number): string | null {
  return cache.runes.get(id) ?? null;
}

/** The lane icon for a role. A mask rather than a picture: the file's shape
 *  is all it carries, and the stylesheet decides its colour. */
export function positionIcon(role: string | null): string | null {
  return role === null ? null : (cache.positions.get(role) ?? null);
}

/** The signed-in account's icon, for the app bar's client card. */
export function profileIcon(id: number | null): string | null {
  return id === null ? null : (cache.profileIcons.get(id) ?? null);
}

/**
 * Fills the cache for one profile icon. Apart from `loadIcons` because it is
 * not row art: it comes from the client poll, one id at a time, and changes
 * only when someone picks a new icon.
 */
export async function loadProfileIcon(id: number): Promise<boolean> {
  if (cache.profileIcons.has(id)) return false;
  let path: string | undefined;
  try {
    const set = await call<IconSet>("resolve_icons", { request: { profileIcons: [id] } });
    path = set.profile_icons?.[id];
  } catch {
    // Offline, or outside the Tauri webview; the card keeps its initial.
  }
  cache.profileIcons.set(id, path ? assetUrl(path) : null);
  return true;
}

/** Everything one page of rows wants drawn. */
interface Wanted {
  champions: Set<string>;
  items: Set<number>;
  spells: Set<string>;
  spellIds: Set<number>;
  runes: Set<number>;
  positions: Set<string>;
}

export function parseScoreboard(json: string | null): Scoreboard | null {
  if (!json) return null;
  try {
    return JSON.parse(json) as Scoreboard;
  } catch {
    // A blob we cannot read is the same as not having one. It was written
    // by a version of this app, so this is close to impossible — and
    // silently rendering the row without a scoreboard is still better than
    // failing the whole library over it.
    return null;
  }
}

function addPage(wanted: Wanted, page: ScoreboardRunes | null | undefined) {
  if (!page) return;
  wanted.runes.add(page.keystone_id);
  wanted.runes.add(page.primary_tree_id);
  wanted.runes.add(page.secondary_tree_id);
}

function collect(rows: RecordingRow[]): Wanted {
  const wanted: Wanted = {
    champions: new Set(),
    items: new Set(),
    spells: new Set(),
    spellIds: new Set(),
    runes: new Set(),
    positions: new Set(),
  };

  for (const row of rows) {
    if (row.champion) wanted.champions.add(row.champion);
    // Five of them at most, ever. Anything the backend does not recognise
    // is a miss, which is the same as an unknown role: no badge.
    if (row.role) wanted.positions.add(row.role);

    const board = parseScoreboard(row.scoreboard_json);

    // Every champion in the game, not only ours: the row draws both team
    // compositions. This is the one set that is bounded by the *game*
    // rather than by the library — there are about 170 champions, a square
    // is around 7 KB, and a library of any size converges on the ones its
    // owner actually meets. Ten times the names is not ten times the disk.
    //
    // Items are asked for the same way, and for the same reason: the row
    // draws the lane opponent's build beside ours. Asking for ours alone
    // left theirs blank except where it happened to share an item with a
    // build of ours somewhere else in the library. Every player rather than
    // `laneOpponent`'s answer, so this cannot disagree with it about who
    // that is — and the set converges on the item catalogue, a few hundred
    // squares, whatever the library's size.
    //
    // Spells and runes follow for the same reason, now that the opponent is
    // drawn in our shape (#341): their spells and their page, not only ours.
    // Both sets are small and bounded by the game, not the library.
    for (const player of board?.players ?? []) {
      if (player.champion) wanted.champions.add(player.champion);
      for (const item of player.items) wanted.items.add(item);
      for (const spell of player.spells) wanted.spells.add(spell);
      for (const id of player.spell_ids ?? []) wanted.spellIds.add(id);
      addPage(wanted, player.runes);
      // A bot laner's boots live in the role slot, not in `items`. Every
      // other role's role slot is a quest token the row never draws, so its
      // art is not asked for.
      if (player.position === "Bottom" && player.role_item != null) {
        wanted.items.add(player.role_item);
      }
    }

    // Older boards carry our page here and nowhere else.
    addPage(wanted, board?.our_runes);
  }
  return wanted;
}

/**
 * Fills the cache for whatever `rows` need and nothing else.
 *
 * Resolves to `true` when anything new arrived, so the caller knows
 * whether repainting would change what is on screen.
 */
export async function loadIcons(rows: RecordingRow[]): Promise<boolean> {
  const wanted = collect(rows);
  const champions = [...wanted.champions].filter((c) => !cache.champions.has(c));
  const items = [...wanted.items].filter((i) => !cache.items.has(i));
  const spells = [...wanted.spells].filter((s) => !cache.spells.has(s));
  const spellIds = [...wanted.spellIds].filter((s) => !cache.spellIds.has(s));
  const runes = [...wanted.runes].filter((r) => !cache.runes.has(r));
  const positions = [...wanted.positions].filter((p) => !cache.positions.has(p));

  if (
    !champions.length &&
    !items.length &&
    !spells.length &&
    !spellIds.length &&
    !runes.length &&
    !positions.length
  ) {
    return false;
  }

  let set: IconSet;
  try {
    set = await call<IconSet>("resolve_icons", {
      request: { champions, items, spells, spellIds, runes, positions },
    });
  } catch {
    // Offline, or the command is unavailable outside the Tauri webview.
    // Record the misses so the same lookup is not retried on every render.
    set = {
      champions: {},
      items: {},
      spells: {},
      spell_ids: {},
      runes: {},
      positions: {},
      profile_icons: {},
    };
  }

  // Everything asked for is recorded, hit or miss. A key the backend left
  // out is one it could not resolve.
  for (const name of champions) {
    cache.champions.set(name, set.champions[name] ? assetUrl(set.champions[name]) : null);
  }
  for (const id of items) {
    cache.items.set(id, set.items[id] ? assetUrl(set.items[id]) : null);
  }
  for (const name of spells) {
    cache.spells.set(name, set.spells[name] ? assetUrl(set.spells[name]) : null);
  }
  for (const id of spellIds) {
    cache.spellIds.set(id, set.spell_ids[id] ? assetUrl(set.spell_ids[id]) : null);
  }
  for (const id of runes) {
    cache.runes.set(id, set.runes[id] ? assetUrl(set.runes[id]) : null);
  }
  for (const role of positions) {
    cache.positions.set(role, set.positions[role] ? assetUrl(set.positions[role]) : null);
  }
  return true;
}
