import { describe, expect, it } from "vitest";
import type { Scoreboard, ScoreboardPlayer, ScoreboardRunes } from "../../types";
import { buildBoxes, runesOf, TRINKET_BOX } from "./build";

function player(over: Partial<ScoreboardPlayer> = {}): ScoreboardPlayer {
  return {
    champion: "Viego",
    team: "ORDER",
    level: 17,
    kills: 0,
    deaths: 0,
    assists: 0,
    cs: 0,
    items: [],
    spells: [],
    ...over,
  };
}

const page = (keystone_id: number): ScoreboardRunes => ({
  keystone_id,
  keystone: "",
  primary_tree_id: 8000,
  secondary_tree_id: 8200,
});

describe("buildBoxes", () => {
  it("puts the trinket in box four and the six items around it", () => {
    const viego = player({ items: [6676, 3036, 6673, 3111, 1038, 1037, 3340], trinket: 3340 });
    expect(buildBoxes(viego, "Jungle")).toEqual([6676, 3036, 6673, 3340, 3111, 1038, 1037]);
    expect(buildBoxes(viego, "Jungle")[TRINKET_BOX]).toBe(3340);
  });

  it("keeps the trinket in box four when the inventory is short", () => {
    // Twitch in the captured game: five items, slot 5 empty.
    const twitch = player({ items: [1086, 6676, 3031, 2512, 3035, 3340], trinket: 3340 });
    expect(buildBoxes(twitch, "Middle")).toEqual([1086, 6676, 3031, 3340, 2512, 3035, null]);
  });

  it("gives a bot laner an eighth box for the role slot's boots", () => {
    const kalista = player({
      items: [3143, 3087, 3124, 3302, 1042, 1086, 3363],
      trinket: 3363,
      role_item: 3008,
    });
    const boxes = buildBoxes(kalista, "Bottom");
    expect(boxes).toHaveLength(8);
    expect(boxes[7]).toBe(3008);
  });

  it("draws the eighth box empty for a bot laner without boots in it yet", () => {
    expect(buildBoxes(player({ items: [3340], trinket: 3340 }), "Bottom")[7]).toBeNull();
  });

  it("never draws another role's role slot, which holds a quest token", () => {
    // 1209 is the jungle quest reward: not an item, and not drawn.
    const boxes = buildBoxes(player({ items: [3340], trinket: 3340, role_item: 1209 }), "Jungle");
    expect(boxes).toHaveLength(7);
    expect(boxes).not.toContain(1209);
  });

  it("finds the trinket on a board written before the field existed", () => {
    const legacy = player({ items: [6676, 3036, 3364] });
    expect(buildBoxes(legacy, "Jungle")).toEqual([6676, 3036, null, 3364, null, null, null]);
  });

  it("does not take a real item for the trinket when the slot was empty", () => {
    const noTrinket = player({ items: [6676, 3036, 6673] });
    expect(buildBoxes(noTrinket, "Jungle")).toEqual([6676, 3036, 6673, null, null, null, null]);
  });

  it("is seven empty boxes for nobody", () => {
    expect(buildBoxes(null, null)).toEqual(Array(7).fill(null));
  });
});

describe("runesOf", () => {
  it("prefers the player's own page", () => {
    const board = { players: [], our_runes: page(9923) } as Scoreboard;
    expect(runesOf(board, player({ is_us: true, runes: page(8010) }))?.keystone_id).toBe(8010);
  });

  it("falls back to our_runes for us on an older board", () => {
    const board = { players: [], our_runes: page(9923) } as Scoreboard;
    expect(runesOf(board, player({ is_us: true }))?.keystone_id).toBe(9923);
  });

  it("gives the opponent nothing on an older board rather than our page", () => {
    const board = { players: [], our_runes: page(9923) } as Scoreboard;
    expect(runesOf(board, player({ is_us: false }))).toBeNull();
  });
});
