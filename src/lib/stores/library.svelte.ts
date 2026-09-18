/**
 * The library's state, as Svelte 5 runes - WS4 task 4.3.
 *
 * One module owns the row set, the disk figure and every control's value,
 * which is the same rule `library.ts` followed ("each module owns exactly one
 * piece of mutable state and is the only place that writes it"). What changes
 * is that the controls are state rather than the DOM being the state: nothing
 * reads a `<select>` to find out what is selected.
 *
 * `$derived` replaces `render()`. The old module recomputed the visible rows
 * and repainted the grid whenever anything changed, and had to defer that
 * work by hand while another view was showing (`pendingRender`). Svelte does
 * not render a component that is not mounted, so that machinery is gone
 * rather than ported.
 */

import { call } from "../../bridge";
import { patchLabel, queueOrModeLabel, vodTitle } from "../../format";
import { toast } from "../../toast";
import type { DiskUsage, ReconcileReport, RecordingRow } from "../../types";
import { ANY, anyFilterActive, filterRows, type LibraryFilters } from "../library/filters";
import { byLane, byName, byPatchDesc, sortRows } from "../library/sort";
import { facetOptions, keepSelection, libraryStats } from "../library/stats";

/** The full set fetched from the DB. Filtering and sorting happen over this
 *  in memory rather than by re-querying: the dataset is small and local. */
let recordings = $state<RecordingRow[]>([]);
let usage = $state<DiskUsage | null>(null);

/** Every control's value. `$state`, so a component binds straight to it. */
const controls = $state({
  champion: "",
  outcome: ANY,
  pinnedOnly: false,
  queue: ANY,
  role: ANY,
  patch: ANY,
  sort: "newest",
});

/**
 * The three derived filters, each paired with what buckets a row and how its
 * values order. The key functions are the same ones `library.ts` built its
 * `Facet` objects from; what has gone is the `HTMLSelectElement` beside them.
 */
const FACETS = [
  {
    id: "queue" as const,
    allLabel: "All queues",
    ariaLabel: "Filter by queue",
    key: (row: RecordingRow) => queueOrModeLabel(row),
    compare: byName,
  },
  {
    id: "role" as const,
    allLabel: "All roles",
    ariaLabel: "Filter by role",
    key: (row: RecordingRow) => row.role,
    compare: byLane,
  },
  {
    id: "patch" as const,
    allLabel: "All patches",
    ariaLabel: "Filter by patch",
    key: (row: RecordingRow) => patchLabel(row.patch),
    compare: byPatchDesc,
  },
];

export const library = {
  get rows() {
    return recordings;
  },
  get usage() {
    return usage;
  },
  get controls() {
    return controls;
  },

  /** The filters as `lib/library/filters` wants them. */
  get filters(): LibraryFilters {
    return {
      champion: controls.champion,
      outcome: controls.outcome,
      pinnedOnly: controls.pinnedOnly,
      facets: FACETS.map((f) => ({ selected: controls[f.id], key: f.key })),
    };
  },

  /** What the grid shows: filtered, then sorted. */
  get visible(): RecordingRow[] {
    return sortRows(filterRows(recordings, this.filters, vodTitle), controls.sort);
  },

  get stats() {
    return libraryStats(this.visible, recordings.length, usage);
  },

  /** Whether anything is narrowing the list, which decides which empty state
   *  the view shows and whether it offers a way out. */
  get filtersActive(): boolean {
    return anyFilterActive(this.filters);
  },

  /**
   * The three derived filters, rebuilt from the rows now in the library.
   *
   * A selection whose value has left the library is reported as `ANY` here
   * rather than being written back to `controls`: a getter that assigns to
   * state is a write during a read, which Svelte would either warn about or
   * loop on. `clearVanishedSelections` does the write, once, after a load.
   */
  get facets() {
    return FACETS.map((facet) => ({
      ...facet,
      ...facetOptions(recordings, facet.key, facet.compare, facet.allLabel, controls[facet.id]),
    }));
  },
};

/** Whether the row set has been loaded at all, as against being empty. */
export function isEmptyLibrary(): boolean {
  return recordings.length === 0;
}

/**
 * Drops any facet selection whose value is no longer in the library.
 *
 * Run after a load rather than inside the getter above. Without it a
 * selection that has left would filter everything out with no way back from
 * the control that made it.
 */
function clearVanishedSelections() {
  for (const facet of library.facets) {
    controls[facet.id] = keepSelection(facet.options, controls[facet.id]);
  }
}

// Every command below reports through `toast`, as `library.ts` did. The store
// is application state, not a pure module: injecting the reporter would put a
// callback at four call sites to avoid one import, and `status.ts` and
// `settings.ts` both call these without a view of their own to report into.

export async function refreshLibrary(): Promise<void> {
  try {
    recordings = await call<RecordingRow[]>("list_recordings");
    clearVanishedSelections();
  } catch (err) {
    toast(`Failed to list recordings: ${err}`, "error");
  }
}

export async function refreshDiskUsage(): Promise<void> {
  try {
    usage = await call<DiskUsage>("get_disk_usage");
  } catch (err) {
    toast(`Failed to load disk usage: ${err}`, "error");
  }
}

export async function rescanRecordings(): Promise<void> {
  try {
    const report = await call<ReconcileReport>("rescan_recordings");
    toast(
      `Rescan complete \u2014 removed ${report.orphans_removed} orphan row(s), ` +
        `imported ${report.imported} untracked file(s).`,
    );
    await Promise.all([refreshLibrary(), refreshDiskUsage()]);
  } catch (err) {
    toast(`Failed to rescan: ${err}`, "error");
  }
}

export async function togglePin(row: RecordingRow): Promise<void> {
  try {
    await call("set_pinned", { recordingId: row.id, pinned: !row.pinned });
    await refreshLibrary();
  } catch (err) {
    toast(`Failed to update pin: ${err}`, "error");
  }
}

export async function deleteRecording(row: RecordingRow): Promise<void> {
  try {
    await call("delete_recording", { recordingId: row.id });
    toast(`Deleted ${vodTitle(row)}.`);
    await Promise.all([refreshLibrary(), refreshDiskUsage()]);
  } catch (err) {
    toast(`Failed to delete: ${err}`, "error");
  }
}

/**
 * Sort is deliberately left alone: it is not a filter, it hides nothing, and
 * resetting it would throw away an order the user chose.
 */
export function clearFilters() {
  controls.champion = "";
  controls.outcome = ANY;
  controls.pinnedOnly = false;
  controls.queue = ANY;
  controls.role = ANY;
  controls.patch = ANY;
}

/** Called once preferences have loaded, which is after the first render. */
export function applyDefaultSort(sort: string) {
  controls.sort = sort;
}
