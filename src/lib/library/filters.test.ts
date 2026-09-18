import { describe, expect, it } from "vitest";
import type { RecordingRow } from "../../types";
import {
  ANY,
  anyFilterActive,
  filterRows,
  type LibraryFilters,
  matchesFilters,
  NONE,
  VALUE_PREFIX,
} from "./filters";

/**
 * These predicates decide what the library shows, and they read their
 * criteria straight off form controls, which is why none of them had a test
 * before WS4.2. The nullable `win` column is the part most likely to be got
 * wrong: a recording with no known outcome is neither a win nor a loss.
 */

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

function filters(over: Partial<LibraryFilters> = {}): LibraryFilters {
  return { champion: "", outcome: ANY, pinnedOnly: false, facets: [], ...over };
}

/** Stands in for `vodTitle`, which has its own fallbacks and its own tests. */
const titleOf = (r: RecordingRow) => r.champion ?? "Unknown recording";

describe("matchesFilters", () => {
  it("keeps everything when nothing is set", () => {
    expect(matchesFilters(row(), filters(), titleOf)).toBe(true);
  });

  describe("the champion box", () => {
    it("matches on a substring, case-insensitively", () => {
      const r = row({ champion: "Ahri" });
      expect(matchesFilters(r, filters({ champion: "ahr" }), titleOf)).toBe(true);
      expect(matchesFilters(r, filters({ champion: "AHR" }), titleOf)).toBe(true);
      expect(matchesFilters(r, filters({ champion: "Zed" }), titleOf)).toBe(false);
    });

    it("ignores surrounding whitespace", () => {
      // Otherwise a stray space empties the library and looks like a bug in
      // the library rather than in the box.
      expect(matchesFilters(row({ champion: "Ahri" }), filters({ champion: "  " }), titleOf)).toBe(
        true,
      );
      expect(
        matchesFilters(row({ champion: "Ahri" }), filters({ champion: " ahri " }), titleOf),
      ).toBe(true);
    });

    it("matches against the title, not the champion column", () => {
      // The title falls back to the filename for a row with no metadata,
      // which is how an imported recording stays findable.
      expect(matchesFilters(row(), filters({ champion: "unknown" }), titleOf)).toBe(true);
    });
  });

  describe("the outcome filter", () => {
    it("separates wins from losses", () => {
      expect(matchesFilters(row({ win: true }), filters({ outcome: "wins" }), titleOf)).toBe(true);
      expect(matchesFilters(row({ win: false }), filters({ outcome: "wins" }), titleOf)).toBe(
        false,
      );
      expect(matchesFilters(row({ win: false }), filters({ outcome: "losses" }), titleOf)).toBe(
        true,
      );
    });

    it("hides a recording with no known outcome from both", () => {
      // `win` is nullable. Written carelessly (`row.win === false`) an
      // unknown outcome would show up under "Losses", which is a claim the
      // data does not support.
      const unknown = row({ win: null });
      expect(matchesFilters(unknown, filters({ outcome: "wins" }), titleOf)).toBe(false);
      expect(matchesFilters(unknown, filters({ outcome: "losses" }), titleOf)).toBe(false);
      expect(matchesFilters(unknown, filters({ outcome: ANY }), titleOf)).toBe(true);
    });
  });

  describe("pinned only", () => {
    it("keeps pinned rows and drops the rest", () => {
      expect(matchesFilters(row({ pinned: true }), filters({ pinnedOnly: true }), titleOf)).toBe(
        true,
      );
      expect(matchesFilters(row({ pinned: false }), filters({ pinnedOnly: true }), titleOf)).toBe(
        false,
      );
    });
  });

  describe("the derived facets", () => {
    const byRole = (selected: string) => ({ selected, key: (r: RecordingRow) => r.role });

    it("does nothing when set to any", () => {
      expect(
        matchesFilters(row({ role: "Top" }), filters({ facets: [byRole(ANY)] }), titleOf),
      ).toBe(true);
    });

    it("matches a named value behind the prefix", () => {
      const f = filters({ facets: [byRole(`${VALUE_PREFIX}Top`)] });
      expect(matchesFilters(row({ role: "Top" }), f, titleOf)).toBe(true);
      expect(matchesFilters(row({ role: "Jungle" }), f, titleOf)).toBe(false);
    });

    it("does not match a row whose column is empty", () => {
      const f = filters({ facets: [byRole(`${VALUE_PREFIX}Top`)] });
      expect(matchesFilters(row({ role: null }), f, titleOf)).toBe(false);
    });

    it("selects exactly the empty rows when set to none", () => {
      const f = filters({ facets: [byRole(NONE)] });
      expect(matchesFilters(row({ role: null }), f, titleOf)).toBe(true);
      expect(matchesFilters(row({ role: "Top" }), f, titleOf)).toBe(false);
    });

    it("requires every facet to agree", () => {
      const f = filters({
        facets: [
          byRole(`${VALUE_PREFIX}Top`),
          { selected: `${VALUE_PREFIX}15.10`, key: (r: RecordingRow) => r.patch },
        ],
      });
      expect(matchesFilters(row({ role: "Top", patch: "15.10" }), f, titleOf)).toBe(true);
      expect(matchesFilters(row({ role: "Top", patch: "15.9" }), f, titleOf)).toBe(false);
    });

    it("cannot be fooled by a value that looks like a reserved one", () => {
      // The prefix exists so a champion or patch literally called "all" or
      // "none" cannot collide with the two reserved option values.
      const f = filters({ facets: [byRole(`${VALUE_PREFIX}${NONE}`)] });
      expect(matchesFilters(row({ role: NONE }), f, titleOf)).toBe(true);
      expect(matchesFilters(row({ role: null }), f, titleOf)).toBe(false);
    });
  });
});

describe("filterRows", () => {
  it("keeps the original order", () => {
    const rows = [row({ id: 1, pinned: true }), row({ id: 2 }), row({ id: 3, pinned: true })];
    expect(filterRows(rows, filters({ pinnedOnly: true }), titleOf).map((r) => r.id)).toEqual([
      1, 3,
    ]);
  });

  it("returns everything when nothing is set", () => {
    const rows = [row({ id: 1 }), row({ id: 2 })];
    expect(filterRows(rows, filters(), titleOf)).toHaveLength(2);
  });
});

describe("anyFilterActive", () => {
  it("is false when nothing narrows the list", () => {
    expect(anyFilterActive(filters())).toBe(false);
    expect(anyFilterActive(filters({ champion: "   " }))).toBe(false);
  });

  it("notices each filter", () => {
    expect(anyFilterActive(filters({ champion: "Ahri" }))).toBe(true);
    expect(anyFilterActive(filters({ outcome: "wins" }))).toBe(true);
    expect(anyFilterActive(filters({ pinnedOnly: true }))).toBe(true);
    expect(anyFilterActive(filters({ facets: [{ selected: NONE, key: (r) => r.role }] }))).toBe(
      true,
    );
  });

  it("does not count the sort order", () => {
    // Sort hides nothing, and counting it would offer to reset an order the
    // user chose. There is no sort field here at all, which is the point.
    expect(anyFilterActive(filters())).toBe(false);
  });
});
