import { describe, expect, it } from "vitest";
import type { RecordingRow } from "../../types";
import { byLane, byName, byPatchDesc, LANE_ORDER, patchParts, sortRows } from "./sort";

function row(over: Partial<RecordingRow> = {}): RecordingRow {
  return {
    id: 1,
    path: "C:/vods/a.mp4",
    started_at: 0,
    duration_s: null,
    game_id: null,
    queue: null,
    champion: null,
    role: null,
    win: null,
    kda_k: null,
    kda_d: null,
    kda_a: null,
    patch: null,
    pinned: false,
    size_bytes: 0,
    audio_tracks_json: null,
    game_mode: null,
    diagnostics_json: null,
    scoreboard_json: null,
    cs: null,
    tier: null,
    division: null,
    lp_after: null,
    lp_before: null,
    lp_delta: null,
    ...over,
  } as RecordingRow;
}

describe("byLane", () => {
  it("uses the game's order, not the alphabet", () => {
    const shuffled = ["Support", "Top", "Bottom", "Jungle", "Middle"];
    expect([...shuffled].sort(byLane)).toEqual(LANE_ORDER);
  });

  it("sorts an unrecognised role after the five rather than dropping it", () => {
    // `role` is a TEXT column and the LCU is not guaranteed to stay its only
    // writer.
    const sorted = [...LANE_ORDER, "Sideways"].sort(byLane);
    expect(sorted[sorted.length - 1]).toBe("Sideways");
  });

  it("falls back to alphabetical between two unrecognised roles", () => {
    expect(["Zebra", "Aardvark"].sort(byLane)).toEqual(["Aardvark", "Zebra"]);
  });
});

describe("patchParts", () => {
  it("splits a version into numbers", () => {
    expect(patchParts("15.10")).toEqual([15, 10]);
  });

  it("refuses anything that is not a run of numbers", () => {
    expect(patchParts("not-a-patch")).toBeNull();
    expect(patchParts("15.x")).toBeNull();
  });
});

describe("byPatchDesc", () => {
  it("sorts newest first", () => {
    expect(["15.9", "15.11", "15.10"].sort(byPatchDesc)).toEqual(["15.11", "15.10", "15.9"]);
  });

  it("compares components as numbers, not as strings", () => {
    // The whole reason this function exists. As strings, "15.9" > "15.10".
    expect(byPatchDesc("15.9", "15.10")).toBeGreaterThan(0);
  });

  it("orders across a major version", () => {
    expect(["14.24", "15.1"].sort(byPatchDesc)).toEqual(["15.1", "14.24"]);
  });

  it("treats a missing component as zero", () => {
    expect(byPatchDesc("15", "15.1")).toBeGreaterThan(0);
  });

  it("falls back to alphabetical for a malformed patch", () => {
    // `patchLabel` passes a malformed patch through untouched, so a label
    // that is not a version still has to order somehow.
    expect(["zzz", "aaa"].sort(byPatchDesc)).toEqual(["aaa", "zzz"]);
  });
});

describe("byName", () => {
  it("compares as text", () => {
    expect(["b", "a"].sort(byName)).toEqual(["a", "b"]);
  });
});

describe("sortRows", () => {
  const older = row({ id: 1, started_at: 100, duration_s: 900, champion: "Zed" });
  const newer = row({ id: 2, started_at: 200, duration_s: 300, champion: "Ahri" });

  it("defaults to newest first", () => {
    expect(sortRows([older, newer], "newest").map((r) => r.id)).toEqual([2, 1]);
    expect(sortRows([older, newer], "anything else").map((r) => r.id)).toEqual([2, 1]);
  });

  it("sorts oldest first", () => {
    expect(sortRows([newer, older], "oldest").map((r) => r.id)).toEqual([1, 2]);
  });

  it("sorts longest first", () => {
    expect(sortRows([newer, older], "longest").map((r) => r.id)).toEqual([1, 2]);
  });

  it("treats an unknown duration as zero rather than dropping the row", () => {
    const unknown = row({ id: 3, duration_s: null });
    expect(sortRows([unknown, older], "longest").map((r) => r.id)).toEqual([1, 3]);
  });

  it("sorts by champion alphabetically", () => {
    expect(sortRows([older, newer], "champion").map((r) => r.id)).toEqual([2, 1]);
  });

  it("puts a row with no champion first rather than crashing", () => {
    const anon = row({ id: 3, champion: null });
    expect(sortRows([older, anon], "champion").map((r) => r.id)).toEqual([3, 1]);
  });

  it("does not reorder the array it was given", () => {
    // The caller's array is the filtered view a previous render is holding.
    const rows = [older, newer];
    sortRows(rows, "oldest");
    expect(rows.map((r) => r.id)).toEqual([1, 2]);
  });
});
