import { beforeEach, describe, expect, it, vi } from "vitest";
import type { RecordingRow } from "../../types";
import { ANY, VALUE_PREFIX } from "../library/filters";

/**
 * The library store's decisions, as against the pure modules it composes.
 *
 * `filters.ts`, `sort.ts` and `stats.ts` are tested on their own. What is here
 * is the part that owns mutable state: which control values survive a load,
 * what a failed command says, and which refreshes each write triggers.
 */

const call = vi.hoisted(() => vi.fn());
vi.mock("../../bridge", () => ({ call, hasDevCommands: vi.fn(), assetUrl: (p: string) => p }));
const toast = vi.hoisted(() => vi.fn());
vi.mock("./toast.svelte", () => ({ toast }));

let store: typeof import("./library.svelte");

const row = (over: Partial<RecordingRow> = {}): RecordingRow =>
  ({
    id: 1,
    path: "C:\\Videos\\a.mp4",
    started_at: 1_700_000_000_000,
    duration_s: 1500,
    size_bytes: 1024,
    champion: "Ahri",
    win: true,
    queue: 420,
    game_mode: "CLASSIC",
    role: "MIDDLE",
    patch: "14.1",
    pinned: false,
    ...over,
  }) as RecordingRow;

/** Answers `list_recordings` with these rows and every other call with null. */
function backend(rows: RecordingRow[], usage: unknown = { total_bytes: 0, free_bytes: 0 }) {
  call.mockImplementation(async (command: string) => {
    if (command === "list_recordings") return rows;
    if (command === "get_disk_usage") return usage;
    return null;
  });
}

beforeEach(async () => {
  vi.resetModules();
  call.mockReset();
  toast.mockReset();
  store = await import("./library.svelte");
});

describe("loading", () => {
  it("starts empty, and says so as a load state rather than a row count", async () => {
    expect(store.isEmptyLibrary()).toBe(true);
    backend([row()]);
    await store.refreshLibrary();
    expect(store.isEmptyLibrary()).toBe(false);
  });

  it("reports a failed list rather than leaving the grid silently stale", async () => {
    call.mockRejectedValue(new Error("no daemon"));
    await store.refreshLibrary();
    expect(toast).toHaveBeenCalledWith(
      expect.stringContaining("Failed to list recordings"),
      "error",
    );
    expect(store.library.rows).toEqual([]);
  });

  it("reports a failed disk read separately, since the grid is still usable", async () => {
    call.mockRejectedValue(new Error("nope"));
    await store.refreshDiskUsage();
    expect(toast).toHaveBeenCalledWith(
      expect.stringContaining("Failed to load disk usage"),
      "error",
    );
    expect(store.library.usage).toBeNull();
  });
});

describe("a selection whose value has left the library", () => {
  it("is dropped on the next load, not merely reported as ANY", async () => {
    backend([row({ id: 1, role: "MIDDLE" }), row({ id: 2, role: "JUNGLE" })]);
    await store.refreshLibrary();
    store.library.controls.role = `${VALUE_PREFIX}JUNGLE`;
    expect(store.library.visible.map((r) => r.id)).toEqual([2]);

    // The jungle game is deleted elsewhere. Without the write-back the filter
    // would hide everything with no way out of the control that set it.
    backend([row({ id: 1, role: "MIDDLE" })]);
    await store.refreshLibrary();

    expect(store.library.controls.role).toBe(ANY);
    expect(store.library.visible.map((r) => r.id)).toEqual([1]);
  });

  it("is kept when it is still there", async () => {
    backend([row({ id: 1, role: "MIDDLE" }), row({ id: 2, role: "JUNGLE" })]);
    await store.refreshLibrary();
    store.library.controls.role = `${VALUE_PREFIX}JUNGLE`;
    await store.refreshLibrary();
    expect(store.library.controls.role).toBe(`${VALUE_PREFIX}JUNGLE`);
  });
});

describe("the controls", () => {
  beforeEach(async () => {
    backend([row({ id: 1, pinned: true }), row({ id: 2, champion: "Sylas" })]);
    await store.refreshLibrary();
  });

  it("filters then sorts, in that order", async () => {
    store.library.controls.sort = "oldest";
    backend([
      row({ id: 1, started_at: 3, champion: "Ahri" }),
      row({ id: 2, started_at: 1, champion: "Sylas" }),
      row({ id: 3, started_at: 2, champion: "Ahri" }),
    ]);
    await store.refreshLibrary();
    store.library.controls.champion = "ahri";
    expect(store.library.visible.map((r) => r.id)).toEqual([3, 1]);
  });

  it("knows when something is narrowing the list", () => {
    expect(store.library.filtersActive).toBe(false);
    store.library.controls.pinnedOnly = true;
    expect(store.library.filtersActive).toBe(true);
  });

  it("clears every filter", () => {
    store.library.controls.champion = "ahri";
    store.library.controls.pinnedOnly = true;
    store.library.controls.role = `${VALUE_PREFIX}MIDDLE`;
    store.clearFilters();
    expect(store.library.filtersActive).toBe(false);
  });

  it("leaves the sort alone when clearing, because it hides nothing", () => {
    store.applyDefaultSort("oldest");
    store.library.controls.champion = "ahri";
    store.clearFilters();
    expect(store.library.controls.sort).toBe("oldest");
  });

  it("offers a facet option per distinct value, plus the all-label", () => {
    const queue = store.library.facets.find((f) => f.id === "queue");
    expect(queue?.allLabel).toBe("All queues");
    expect(queue?.options.length).toBeGreaterThan(0);
  });

  it("says how many of the held rows are showing, and only when it is fewer", () => {
    expect(store.library.stats.games).toBe("2");
    expect(store.library.stats.gamesSub).toBe("");

    store.library.controls.champion = "sylas";
    expect(store.library.stats.games).toBe("1");
    expect(store.library.stats.gamesSub).toBe("of 2");
  });
});

describe("the writes", () => {
  beforeEach(async () => {
    backend([row({ id: 7, pinned: false })]);
    await store.refreshLibrary();
    call.mockClear();
    toast.mockClear();
  });

  it("toggles a pin to the opposite of what the row says", async () => {
    await store.togglePin(row({ id: 7, pinned: false }));
    expect(call).toHaveBeenCalledWith("set_pinned", { recordingId: 7, pinned: true });
    await store.togglePin(row({ id: 7, pinned: true }));
    expect(call).toHaveBeenCalledWith("set_pinned", { recordingId: 7, pinned: false });
  });

  it("re-reads the library after a pin, so the row redraws", async () => {
    await store.togglePin(row({ id: 7 }));
    expect(call).toHaveBeenCalledWith("list_recordings");
  });

  it("names what it deleted, and re-reads the disk figure too", async () => {
    await store.deleteRecording(row({ id: 7, champion: "Ahri" }));
    expect(call).toHaveBeenCalledWith("delete_recording", { recordingId: 7 });
    expect(toast).toHaveBeenCalledWith(expect.stringContaining("Ahri"));
    expect(call).toHaveBeenCalledWith("get_disk_usage");
  });

  it("says what a rescan did in both directions", async () => {
    call.mockImplementation(async (command: string) => {
      if (command === "rescan_recordings") return { orphans_removed: 2, imported: 3 };
      if (command === "list_recordings") return [];
      return null;
    });
    await store.rescanRecordings();
    const message = String(toast.mock.calls[0][0]);
    expect(message).toContain("2 orphan row(s)");
    expect(message).toContain("3 untracked file(s)");
  });

  it("reports a failed write instead of leaving the row looking changed", async () => {
    call.mockRejectedValue(new Error("locked"));
    await store.togglePin(row({ id: 7 }));
    expect(toast).toHaveBeenCalledWith(expect.stringContaining("Failed to update pin"), "error");

    toast.mockClear();
    await store.deleteRecording(row({ id: 7 }));
    expect(toast).toHaveBeenCalledWith(expect.stringContaining("Failed to delete"), "error");

    toast.mockClear();
    await store.rescanRecordings();
    expect(toast).toHaveBeenCalledWith(expect.stringContaining("Failed to rescan"), "error");
  });
});
