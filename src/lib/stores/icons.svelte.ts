/**
 * Art, made reactive - WS4 task 4.3.
 *
 * `icons.ts` owns the cache and the lookups, and both are synchronous: they
 * answer from what has already been resolved and return null otherwise.
 * Nothing about that changes here. What this adds is a way for a component to
 * re-read them when the cache grows.
 *
 * **Why a version counter rather than reactive maps.** The cache is keyed five
 * different ways and is written from one place; wrapping each map in `$state`
 * would make five reactive structures to keep in step for a signal that is
 * always the same one, "more art exists now". A component reads `version`
 * before calling the plain lookup, so it re-runs when that fires and does no
 * work in between.
 *
 * This is still the two-pass design `library.ts` used, for the reason it
 * gives: the first sighting of any icon is a CDN round trip, so blocking the
 * list on it would trade a library that renders instantly for one that renders
 * once the network says so, and the whole thing has to work with no network at
 * all, where the answer is "no art" and every row is already correct without
 * it.
 */

import { loadIcons } from "../../icons";
import type { RecordingRow } from "../../types";

let version = $state(0);

/**
 * Read this before any `icons.ts` lookup, and the lookup re-runs when new art
 * lands. The number itself means nothing.
 */
export function iconVersion(): number {
  return version;
}

/** How many rows to resolve art for before repainting what has arrived. */
const ART_CHUNK = 8;

/**
 * Resolves the art `rows` need, a chunk at a time, top down.
 *
 * Chunked so the rows a person is actually looking at fill in first. The total
 * wait is the same; what changes is that it stops being one wait for
 * everything. On a warm cache every chunk resolves without a request and this
 * is indistinguishable from a single pass.
 */
export async function fillInArt(rows: readonly RecordingRow[]): Promise<void> {
  for (let i = 0; i < rows.length; i += ART_CHUNK) {
    if (await loadIcons(rows.slice(i, i + ART_CHUNK))) version += 1;
  }
}
