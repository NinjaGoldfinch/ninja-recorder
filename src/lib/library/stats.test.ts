import { describe, expect, it } from "vitest";
import type { DiskUsage, RecordingRow } from "../../types";
import { ANY, NONE, VALUE_PREFIX } from "./filters";
import { byName } from "./sort";
import { facetOptions, keepSelection, libraryStats } from "./stats";

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

describe("libraryStats", () => {
  it("counts the visible rows and names the total when a filter narrows", () => {
    // Otherwise "100%" under Wins-only reads as a perfect record.
    expect(libraryStats([row()], 1, null).gamesSub).toBe("");
    expect(libraryStats([row()], 5, null).gamesSub).toBe("of 5");
  });

  it("excludes undecided games from the win rate", () => {
    // `win` is null for anything reconcile imported: it knows the path and the
    // size and nothing else. Counting that as a loss understates the rate.
    const rows = [row({ win: true }), row({ win: false }), row({ win: null })];
    const stats = libraryStats(rows, 3, null);
    expect(stats.winrate).toBe("50%");
    expect(stats.winrateSub).toBe("1W 1L");
  });

  it("says so when there is no result to rate", () => {
    const stats = libraryStats([row({ win: null })], 1, null);
    expect(stats.winrate).toBe("\u2014");
    expect(stats.winrateSub).toBe("no results yet");
  });

  it("totals only the durations it has, and counts the ones it does not", () => {
    const rows = [row({ duration_s: 600 }), row({ duration_s: null })];
    const stats = libraryStats(rows, 2, null);
    expect(stats.playtime).toBe(600);
    expect(stats.playtimeSub).toBe("1 unknown");
  });

  it("leaves the playtime sub-label empty when every row is timed", () => {
    expect(libraryStats([row({ duration_s: 60 })], 1, null).playtimeSub).toBe("");
  });

  it("reports free space only once disk usage has been read", () => {
    expect(libraryStats([], 0, null).diskFreeBytes).toBeNull();
    const usage = { free_bytes: 1234, total_bytes: 0, used_bytes: 0 } as unknown as DiskUsage;
    expect(libraryStats([], 0, usage).diskFreeBytes).toBe(1234);
  });

  it("sums the sizes of the visible rows", () => {
    expect(libraryStats([row({ size_bytes: 10 }), row({ size_bytes: 5 })], 2, null).diskBytes).toBe(
      15,
    );
  });
});

describe("facetOptions", () => {
  const roleOf = (r: RecordingRow) => r.role;

  it("offers All first, then the values present", () => {
    const rows = [row({ role: "Top" }), row({ role: "Jungle" }), row({ role: "Top" })];
    const { options } = facetOptions(rows, roleOf, byName, "All roles", ANY);
    expect(options.map((o) => o.label)).toEqual(["All roles", "Jungle", "Top"]);
    expect(options[1].value).toBe(`${VALUE_PREFIX}Jungle`);
  });

  it("offers Unknown only when something is missing", () => {
    const withGap = facetOptions([row({ role: "Top" }), row()], roleOf, byName, "All", ANY);
    expect(withGap.options.map((o) => o.value)).toContain(NONE);

    const without = facetOptions([row({ role: "Top" })], roleOf, byName, "All", ANY);
    expect(without.options.map((o) => o.value)).not.toContain(NONE);
  });

  it("disables a control with only one choice", () => {
    // One choice is no choice: a library of nothing but ARAM has no queue to
    // pick between.
    const { disabled } = facetOptions([row({ role: "Top" })], roleOf, byName, "All", ANY);
    expect(disabled).toBe(true);
  });

  it("never disables a control that is currently filtering", () => {
    // Retention or a delete can take the library down to the one value already
    // selected, and greying the control there would leave the selection with
    // no way to undo it from the control that made it.
    const { disabled } = facetOptions(
      [row({ role: "Top" })],
      roleOf,
      byName,
      "All",
      `${VALUE_PREFIX}Top`,
    );
    expect(disabled).toBe(false);
  });
});

describe("keepSelection", () => {
  it("keeps a selection whose value is still there", () => {
    const options = [
      { value: ANY, label: "All" },
      { value: `${VALUE_PREFIX}Top`, label: "Top" },
    ];
    expect(keepSelection(options, `${VALUE_PREFIX}Top`)).toBe(`${VALUE_PREFIX}Top`);
  });

  it("falls back to All when the value has left the library", () => {
    // Otherwise the selection quietly filters everything out.
    expect(keepSelection([{ value: ANY, label: "All" }], `${VALUE_PREFIX}Gone`)).toBe(ANY);
  });
});
