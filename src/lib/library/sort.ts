/**
 * Ordering for the library's facet lists and its row list.
 *
 * Extracted from `library.ts` by WS4.2. The comparators were already pure;
 * what they lacked was a test, and `byPatchDesc` in particular is the kind of
 * thing that looks right and is not.
 */

import type { RecordingRow } from "../../types";

export function byName(a: string, b: string): number {
  return a.localeCompare(b);
}

/**
 * The order the game lists lanes in, not alphabetical.
 *
 * Nobody scans a lane picker for "Bottom, Jungle, Middle, Support, Top".
 */
export const LANE_ORDER = ["Top", "Jungle", "Middle", "Bottom", "Support"];

/**
 * Lane order, with anything unrecognised sorted after the five rather than
 * dropped.
 *
 * `position()` only ever writes those five, but `role` is a TEXT column and
 * the LCU is not guaranteed to stay its only writer.
 */
export function byLane(a: string, b: string): number {
  const ia = LANE_ORDER.indexOf(a);
  const ib = LANE_ORDER.indexOf(b);
  if (ia === -1 && ib === -1) return byName(a, b);
  if (ia === -1) return 1;
  if (ib === -1) return -1;
  return ia - ib;
}

/** `"15.10"` to `[15, 10]`, or null if it is not a run of numbers. */
export function patchParts(label: string): number[] | null {
  const parts = label.split(".").map(Number);
  return parts.every((n) => Number.isFinite(n)) ? parts : null;
}

/**
 * Newest patch first, compared component-wise as numbers.
 *
 * **"15.9" sorts older than "15.10"**, which is exactly what comparing them
 * as strings gets wrong. `patchLabel` passes a malformed patch through
 * untouched, so a label that is not a version still has to order somehow, and
 * falls back to alphabetical.
 */
export function byPatchDesc(a: string, b: string): number {
  const pa = patchParts(a);
  const pb = patchParts(b);
  if (pa === null || pb === null) return byName(a, b);
  for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
    const diff = (pb[i] ?? 0) - (pa[i] ?? 0);
    if (diff !== 0) return diff;
  }
  return 0;
}

/** The values `#sort-select` offers. Anything else means newest first. */
export type SortOrder = "newest" | "oldest" | "longest" | "champion";

/**
 * Orders the visible rows.
 *
 * Copies rather than sorting in place: the caller's array is the filtered
 * view of the library, and `Array.prototype.sort` mutating it would reorder
 * the rows a previous render is still holding.
 */
export function sortRows(rows: readonly RecordingRow[], order: string): RecordingRow[] {
  return [...rows].sort((a, b) => {
    switch (order) {
      case "oldest":
        return a.started_at - b.started_at;
      case "longest":
        return (b.duration_s ?? 0) - (a.duration_s ?? 0);
      case "champion":
        return (a.champion ?? "").localeCompare(b.champion ?? "");
      default:
        return b.started_at - a.started_at;
    }
  });
}
