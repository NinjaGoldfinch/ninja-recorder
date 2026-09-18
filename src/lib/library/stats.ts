/**
 * The four numbers above the library, and the derived filters beside it.
 *
 * Both were computed inside functions that immediately wrote them to
 * elements (`renderStats`, `refreshFacets`), which is why neither had a test.
 * WS4.3 needs them as values rather than as side effects, because a Svelte
 * component renders what it is given.
 */

import type { DiskUsage, RecordingRow } from "../../types";
import { ANY, NONE, VALUE_PREFIX } from "./filters";

export interface LibraryStats {
  games: string;
  gamesSub: string;
  winrate: string;
  winrateSub: string;
  playtime: number;
  playtimeSub: string;
  diskBytes: number;
  /** Null when disk usage has not been read yet, which is a blank sub-label. */
  diskFreeBytes: number | null;
}

/**
 * Computed over the **filtered** rows, so the numbers track the filters, with
 * a sub-label naming the total whenever a filter is narrowing things.
 * Otherwise "100%" under Wins-only reads as a perfect record.
 *
 * `formatSpan` and `formatBytes` are deliberately not applied here: this
 * returns the numbers and the component formats them, so a test can assert
 * "2 of 5 wins" without asserting a string that belongs to `format.ts`.
 */
export function libraryStats(
  visible: readonly RecordingRow[],
  total: number,
  usage: DiskUsage | null,
): LibraryStats {
  // `win` is null for anything reconcile imported: it only knows the path and
  // the size. Treating that as a loss would quietly understate the rate.
  const decided = visible.filter((r) => r.win !== null);
  const wins = decided.filter((r) => r.win === true).length;

  // Same story for `duration_s`.
  const timed = visible.filter((r) => r.duration_s !== null);

  return {
    games: String(visible.length),
    gamesSub: visible.length === total ? "" : `of ${total}`,
    winrate: decided.length === 0 ? "—" : `${Math.round((wins / decided.length) * 100)}%`,
    winrateSub: decided.length === 0 ? "no results yet" : `${wins}W ${decided.length - wins}L`,
    playtime: timed.reduce((sum, r) => sum + (r.duration_s ?? 0), 0),
    playtimeSub: timed.length === visible.length ? "" : `${visible.length - timed.length} unknown`,
    // `size_bytes` is NOT NULL DEFAULT 0, so no null handling here.
    diskBytes: visible.reduce((sum, r) => sum + r.size_bytes, 0),
    diskFreeBytes: usage?.free_bytes ?? null,
  };
}

export interface FacetOption {
  /** The control's value: `ANY`, `NONE`, or a name behind `VALUE_PREFIX`. */
  value: string;
  label: string;
}

export interface FacetOptions {
  options: FacetOption[];
  /**
   * Whether the control should be greyed out.
   *
   * **One choice is no choice** — a library of nothing but ARAM has no queue
   * to pick between. But a facet that is *currently* filtering is never
   * disabled: retention or a delete can take the library down to the one
   * value already selected, and greying the control there would leave the
   * selection with no way to undo it from the control that made it.
   */
  disabled: boolean;
}

/**
 * Rebuilds one derived filter's options from the rows now in the library.
 *
 * `allLabel` is the option that lives in the markup rather than in the data
 * ("All queues"), and is always first.
 */
export function facetOptions(
  rows: readonly RecordingRow[],
  key: (row: RecordingRow) => string | null,
  compare: (a: string, b: string) => number,
  allLabel: string,
  selected: string,
): FacetOptions {
  const values = new Set<string>();
  let unknowns = false;
  for (const row of rows) {
    const value = key(row);
    if (value === null) unknowns = true;
    else values.add(value);
  }

  const options: FacetOption[] = [{ value: ANY, label: allLabel }];
  for (const value of [...values].sort(compare)) {
    options.push({ value: VALUE_PREFIX + value, label: value });
  }
  // "Unknown" is worth offering, and only when something is missing: "which
  // of my games never got a role" is the question the `Unknown` on the row
  // itself prompts, and the backfill leaves plenty of them.
  if (unknowns) options.push({ value: NONE, label: "Unknown" });

  return { options, disabled: options.length < 3 && selected === ANY };
}

/**
 * The selection to keep, given the options that now exist.
 *
 * A selection whose value has left the library falls back to `ANY` rather
 * than quietly filtering everything out.
 */
export function keepSelection(options: readonly FacetOption[], previous: string): string {
  return options.some((o) => o.value === previous) ? previous : ANY;
}
