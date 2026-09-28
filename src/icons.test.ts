import { beforeEach, describe, expect, it, vi } from "vitest";
import type { RecordingRow, Scoreboard, ScoreboardPlayer } from "./types";

const call = vi.fn();
vi.mock("./bridge", () => ({ call, assetUrl: (p: string) => p }));

const { itemIcon, loadIcons, positionIcon } = await import("./icons");

const player = (over: Partial<ScoreboardPlayer>): ScoreboardPlayer => ({
  champion: "Viego",
  team: "ORDER",
  level: 18,
  kills: 0,
  deaths: 0,
  assists: 0,
  cs: 0,
  items: [],
  spells: [],
  ...over,
});

const withBoard = (board: Scoreboard): RecordingRow =>
  ({ champion: "Viego", scoreboard_json: JSON.stringify(board) }) as RecordingRow;

describe("loadIcons", () => {
  beforeEach(() => call.mockReset());

  // The matchup draws the opponent's build. Asking for our items alone left
  // every slot of theirs blank that did not happen to match one of ours.
  it("asks for the opponent's items, not only ours", async () => {
    call.mockResolvedValue({
      champions: {},
      items: { 3111: "3111.png", 6692: "6692.png" },
      spells: {},
      spell_ids: {},
      runes: {},
      positions: {},
    });
    const row = withBoard({
      players: [
        player({ is_us: true, position: "Jungle", items: [3111] }),
        player({ champion: "Lillia", team: "CHAOS", position: "Jungle", items: [6692] }),
      ],
    });

    await loadIcons([row]);

    const { request } = call.mock.calls[0][1] as { request: { items: number[] } };
    expect(request.items).toEqual(expect.arrayContaining([3111, 6692]));
    expect(itemIcon(6692)).toBe("6692.png");
  });

  // The role badge is the one piece of art that is not Data Dragon's, and it
  // rides the same call rather than a second one.
  it("asks for our role's icon in the same call, and remembers a miss", async () => {
    call.mockResolvedValue({
      champions: {},
      items: {},
      spells: {},
      spell_ids: {},
      runes: {},
      positions: { Jungle: "cdragon/16.19/position/position-jungle.svg" },
    });
    const rows = [
      { champion: "Viego", role: "Jungle", scoreboard_json: null },
      { champion: "Viego", role: "Support", scoreboard_json: null },
      { champion: "Viego", role: null, scoreboard_json: null },
    ] as RecordingRow[];

    await loadIcons(rows);

    const { request } = call.mock.calls[0][1] as { request: { positions: string[] } };
    expect(request.positions.sort()).toEqual(["Jungle", "Support"]);
    expect(positionIcon("Jungle")).toBe("cdragon/16.19/position/position-jungle.svg");
    expect(positionIcon("Support")).toBeNull();
    expect(positionIcon(null)).toBeNull();

    // Both were answered, one of them with a miss, so neither is asked again.
    call.mockClear();
    expect(await loadIcons(rows)).toBe(false);
    expect(call).not.toHaveBeenCalled();
  });
});
