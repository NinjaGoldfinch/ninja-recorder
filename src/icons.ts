/**
 * Data Dragon art, asked for once per page rather than once per icon.
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
import type { IconSet, RecordingRow, Scoreboard } from "./types";

/** Resolved paths, and the misses. A `null` is "asked and not found", which
 *  is different from "not asked yet" and stops a missing icon being
 *  re-requested on every render. */
const cache = {
  champions: new Map<string, string | null>(),
  items: new Map<number, string | null>(),
  spells: new Map<string, string | null>(),
  runes: new Map<number, string | null>(),
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

export function runeIcon(id: number): string | null {
  return cache.runes.get(id) ?? null;
}

/** Everything one page of rows wants drawn. */
interface Wanted {
  champions: Set<string>;
  items: Set<number>;
  spells: Set<string>;
  runes: Set<number>;
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

function collect(rows: RecordingRow[]): Wanted {
  const wanted: Wanted = {
    champions: new Set(),
    items: new Set(),
    spells: new Set(),
    runes: new Set(),
  };

  for (const row of rows) {
    if (row.champion) wanted.champions.add(row.champion);

    const board = parseScoreboard(row.scoreboard_json);
    const us = board?.players.find((p) => p.is_us);
    if (!us) continue;

    for (const item of us.items) wanted.items.add(item);
    for (const spell of us.spells) wanted.spells.add(spell);
    if (board?.our_runes) {
      wanted.runes.add(board.our_runes.keystone_id);
      wanted.runes.add(board.our_runes.primary_tree_id);
      wanted.runes.add(board.our_runes.secondary_tree_id);
    }
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
  const runes = [...wanted.runes].filter((r) => !cache.runes.has(r));

  if (!champions.length && !items.length && !spells.length && !runes.length) {
    return false;
  }

  let set: IconSet;
  try {
    set = await call<IconSet>("resolve_icons", {
      request: { champions, items, spells, runes },
    });
  } catch {
    // Offline, or the command is unavailable outside the Tauri webview.
    // Record the misses so the same lookup is not retried on every render.
    set = { champions: {}, items: {}, spells: {}, runes: {} };
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
  for (const id of runes) {
    cache.runes.set(id, set.runes[id] ? assetUrl(set.runes[id]) : null);
  }
  return true;
}
