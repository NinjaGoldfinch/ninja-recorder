/**
 * Which recordings the library shows.
 *
 * Extracted from `library.ts`'s `visibleRows` and `filtersActive` by WS4.2.
 * Both read their criteria straight off the DOM (`els.champion.value`,
 * `facet.select.value`), which is why neither had a test: asking "does a
 * pinned-only filter hide an unpinned row" meant building a settings form.
 *
 * The criteria are plain data here. `library.ts` reads the controls and
 * builds one of these; nothing in this file knows an element exists.
 */

import type { RecordingRow } from "../../types";

/** Every named facet value carries this prefix, so no label can collide with
 *  the two reserved values below. */
export const VALUE_PREFIX = "v:";
/** No filtering on this facet. Shared with `#filter-outcome`'s own markup. */
export const ANY = "all";
/** Rows whose column is empty. Offered only when there are some. */
export const NONE = "none";

/**
 * One derived filter: queue, role or patch.
 *
 * `key` is what buckets a row, returning null when the row's column is empty.
 * `selected` is the raw control value, so it is `ANY`, `NONE`, or a name
 * behind `VALUE_PREFIX`.
 */
export interface FacetSelection {
  selected: string;
  key: (row: RecordingRow) => string | null;
}

export interface LibraryFilters {
  /** Free text, matched against the row's title. Trimmed and lowercased here. */
  champion: string;
  /** `ANY`, `"wins"` or `"losses"`. */
  outcome: string;
  pinnedOnly: boolean;
  facets: readonly FacetSelection[];
}

/**
 * Whether one row survives the filters.
 *
 * `titleOf` is `format.ts`'s `vodTitle`, injected rather than imported so
 * this module stays free of the fallback-to-filename logic and can be tested
 * against a title that says what the test means.
 *
 * Note `row.win !== true` rather than `row.win === false`: `win` is nullable,
 * and a recording with no known outcome is neither a win nor a loss, so it is
 * hidden by both filters instead of showing up under whichever one is
 * written carelessly.
 */
export function matchesFilters(
  row: RecordingRow,
  filters: LibraryFilters,
  titleOf: (row: RecordingRow) => string,
): boolean {
  const champion = filters.champion.trim().toLowerCase();
  if (champion && !titleOf(row).toLowerCase().includes(champion)) return false;

  if (filters.outcome === "wins" && row.win !== true) return false;
  if (filters.outcome === "losses" && row.win !== false) return false;

  if (filters.pinnedOnly && !row.pinned) return false;

  for (const facet of filters.facets) {
    if (facet.selected === ANY) continue;
    const key = facet.key(row);
    if (facet.selected === NONE) {
      if (key !== null) return false;
    } else if (key === null || VALUE_PREFIX + key !== facet.selected) {
      return false;
    }
  }

  return true;
}

/** The rows that survive the filters, in their original order. */
export function filterRows(
  rows: readonly RecordingRow[],
  filters: LibraryFilters,
  titleOf: (row: RecordingRow) => string,
): RecordingRow[] {
  return rows.filter((row) => matchesFilters(row, filters, titleOf));
}

/**
 * Whether anything is narrowing the list, which is what decides if the
 * "no matches" state offers a way out.
 *
 * **Sort is deliberately not a filter here.** It hides nothing, and counting
 * it would offer to reset an order the user chose.
 */
export function anyFilterActive(filters: LibraryFilters): boolean {
  return (
    filters.champion.trim() !== "" ||
    filters.outcome !== ANY ||
    filters.pinnedOnly ||
    filters.facets.some((facet) => facet.selected !== ANY)
  );
}
