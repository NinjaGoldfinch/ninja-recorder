/**
 * The Events tab's list: which markers a filter keeps, how a burst of the
 * same event collapses into one row, and which row the playhead is on.
 *
 * Pure, and separate from `markers.ts`, which says what one marker looks like;
 * this is about a list of them.
 */

import type { MarkerRow } from "../../types";
import { markerLabel } from "./markers";

export type EventFilter = "all" | "kills" | "deaths" | "objectives";

/**
 * Fights, deaths and map objectives. Assists and aces count as fights, since
 * a fight is what they are. A kind no filter names, such as one a newer build
 * writes, still shows under All.
 */
const FILTER_KINDS: Record<Exclude<EventFilter, "all">, ReadonlySet<string>> = {
  kills: new Set(["kill", "assist", "multikill", "first_blood", "ace"]),
  deaths: new Set(["death"]),
  objectives: new Set(["dragon", "baron", "herald", "voidgrubs", "turret", "inhibitor"]),
};

export function matchesFilter(kind: string, filter: EventFilter): boolean {
  return filter === "all" || FILTER_KINDS[filter].has(kind);
}

/**
 * How close together, in game seconds, two identical events have to be to
 * count as one burst: three voidgrubs taken in one go, or a turret's plates.
 */
export const BURST_GAP_S = 45;

export interface EventRow {
  /** The first event in the burst: the one a click seeks to. */
  marker: MarkerRow;
  label: string;
  count: number;
  /** Past the end of the file, so not seekable. */
  beyond: boolean;
}

/**
 * The rows the list shows under `filter`, in order.
 *
 * A burst is only grouped when the label is the same, not merely the kind:
 * "Killed Vayne" twice in a minute is two different fights, whereas
 * "Voidgrubs" three times is one objective.
 */
export function eventRows(
  markers: readonly MarkerRow[],
  beyondIds: ReadonlySet<number>,
  filter: EventFilter,
): EventRow[] {
  const rows: EventRow[] = [];
  let last: MarkerRow | null = null;
  for (const marker of markers) {
    if (!matchesFilter(marker.kind, filter)) continue;
    const label = markerLabel(marker);
    const beyond = beyondIds.has(marker.id);
    const open = rows[rows.length - 1];
    if (
      open &&
      last &&
      open.label === label &&
      open.beyond === beyond &&
      marker.game_time_s - last.game_time_s <= BURST_GAP_S
    ) {
      open.count += 1;
    } else {
      rows.push({ marker, label, count: 1, beyond });
    }
    last = marker;
  }
  return rows;
}

/**
 * The row the playhead has most recently passed, or -1 before the first.
 *
 * Half a second of slack, because seeking to a marker lands a hair before it
 * and the row just clicked should be the one lit.
 */
export function currentRow(rows: readonly EventRow[], videoTimeS: number): number {
  let current = -1;
  rows.forEach((row, i) => {
    if (!row.beyond && row.marker.video_time_s <= videoTimeS + 0.5) current = i;
  });
  return current;
}
