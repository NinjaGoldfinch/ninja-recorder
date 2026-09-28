import { describe, expect, it } from "vitest";
import type { GameReview } from "../contract/types";
import { EMPTY_REVIEW, prefill } from "./autofill";

const game = (over: Partial<GameReview> = {}): GameReview => ({
  game: {
    id: 1,
    recording_id: 1,
    started_at: 0,
    ended_at: null,
    block_id: null,
    champion: "Viego",
    matchup: "Talon",
    result: "win",
    recording_offset_ms: null,
  },
  review: null,
  death_markers: 3,
  objectives: [],
  takeaways: [],
  notes: [],
  ...over,
});

describe("prefill", () => {
  it("fills the game rating from the result and deaths from the stats", () => {
    const { draft, auto } = prefill(game(), { deaths: 2 });
    expect(draft.game_rating).toBe("win");
    expect(draft.deaths).toBe(2);
    expect(auto).toEqual(["game_rating", "deaths"]);
  });

  it("leaves what it does not know blank", () => {
    const loaded = game();
    loaded.game.result = null;
    const { draft, auto } = prefill(loaded, { deaths: null });
    expect(draft).toEqual(EMPTY_REVIEW);
    expect(auto).toEqual([]);
  });

  it("never touches a saved review, blanks included", () => {
    const saved = { ...EMPTY_REVIEW, lane_rating: "win" as const };
    const { draft, auto } = prefill(game({ review: saved }), { deaths: 4 });
    expect(draft).toEqual(saved);
    expect(draft.game_rating).toBeNull();
    expect(auto).toEqual([]);
  });

  it("ignores a death count that is not one", () => {
    expect(prefill(game(), { deaths: -1 }).draft.deaths).toBeNull();
    expect(prefill(game(), { deaths: 1.5 }).draft.deaths).toBeNull();
  });

  it("has nothing to fill without a game", () => {
    expect(prefill(null, { deaths: 2 })).toEqual({ draft: EMPTY_REVIEW, auto: [] });
  });
});
