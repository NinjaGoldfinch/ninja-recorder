import { describe, expect, it } from "vitest";
import type { RecordingRow, Scoreboard, ScoreboardPlayer } from "../../types";
import { csPerMinute, laneOpponent, outcomeAttr, selfPlayer } from "./scoreboard";

function player(over: Partial<ScoreboardPlayer> = {}): ScoreboardPlayer {
  return {
    champion: "Ahri",
    team: "ORDER",
    level: 18,
    kills: 0,
    deaths: 0,
    assists: 0,
    cs: 0,
    items: [],
    spells: [],
    ...over,
  };
}

function withBoard(board: Scoreboard | null): RecordingRow {
  return {
    scoreboard_json: board === null ? null : JSON.stringify(board),
    cs: null,
    duration_s: null,
  } as RecordingRow;
}

describe("laneOpponent", () => {
  it("finds the enemy in our position", () => {
    const row = withBoard({
      players: [
        player({ champion: "Ahri", team: "ORDER", position: "Middle", is_us: true }),
        player({ champion: "Zed", team: "CHAOS", position: "Middle" }),
        player({ champion: "Darius", team: "CHAOS", position: "Top" }),
      ],
    });
    expect(laneOpponent(row)?.champion).toBe("Zed");
  });

  it("is null when positions are missing", () => {
    // Every recording made before the field existed, and any mode with no
    // positions to assign. An arbitrary enemy is worse than none.
    const row = withBoard({
      players: [
        player({ champion: "Ahri", team: "ORDER", is_us: true }),
        player({ champion: "Zed", team: "CHAOS" }),
      ],
    });
    expect(laneOpponent(row)).toBeNull();
  });

  it("is null when nobody on the board is us", () => {
    const row = withBoard({ players: [player({ champion: "Zed", team: "CHAOS" })] });
    expect(laneOpponent(row)).toBeNull();
  });

  it("is null with no scoreboard at all", () => {
    expect(laneOpponent(withBoard(null))).toBeNull();
  });

  it("does not return a team-mate who shares our position", () => {
    // The filter is on team *and* position, not position alone.
    const row = withBoard({
      players: [
        player({ champion: "Ahri", team: "ORDER", position: "Middle", is_us: true }),
        player({ champion: "Lux", team: "ORDER", position: "Middle" }),
      ],
    });
    expect(laneOpponent(row)).toBeNull();
  });
});

describe("selfPlayer", () => {
  it("finds us", () => {
    const row = withBoard({
      players: [player({ champion: "Zed" }), player({ champion: "Ahri", is_us: true })],
    });
    expect(selfPlayer(row)?.champion).toBe("Ahri");
  });

  it("is null with no scoreboard", () => {
    expect(selfPlayer(withBoard(null))).toBeNull();
  });
});

describe("csPerMinute", () => {
  it("divides creep score by the length in minutes", () => {
    expect(csPerMinute({ cs: 200, duration_s: 1200 } as RecordingRow)).toBe("10.0 /min");
  });

  it("is null when either half is missing", () => {
    expect(csPerMinute({ cs: null, duration_s: 1200 } as RecordingRow)).toBeNull();
    expect(csPerMinute({ cs: 200, duration_s: null } as RecordingRow)).toBeNull();
  });

  it("refuses to divide by zero", () => {
    expect(csPerMinute({ cs: 200, duration_s: 0 } as RecordingRow)).toBeNull();
  });
});

describe("outcomeAttr", () => {
  it("separates the three states", () => {
    expect(outcomeAttr(true)).toBe("win");
    expect(outcomeAttr(false)).toBe("loss");
    // Not "loss": an undecided game must not read as one.
    expect(outcomeAttr(null)).toBe("unknown");
  });
});
