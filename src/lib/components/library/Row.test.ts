import { mount, unmount } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { RecordingRow } from "../../../types";
import Row from "./Row.svelte";

/**
 * Parity for the row, against `frontend.md`'s "What a row says when the data is
 * missing" table.
 *
 * Match metadata arrives from two independent sources, so a row can carry
 * either, both or neither, and **a row that hid the slot when a value is
 * missing would be a different shape per recording**. That is the property
 * these tests are about: every column is still there, saying so.
 *
 * They also pin the thing the migration exists for. `library.ts` built this
 * row by concatenating HTML and escaping by hand; a filename reaches the title
 * through `vodTitle` and `db::reconcile` imports whatever the user drops in the
 * folder. The last test is the one that would catch an `{@html}` creeping back.
 */

function row(over: Partial<RecordingRow> = {}): RecordingRow {
  return {
    id: 1,
    path: "C:/vods/recording-1.mp4",
    started_at: Date.now(),
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
    size_bytes: 1024,
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

const noop = () => {};
let host: HTMLElement | null = null;
let instance: Record<string, unknown> | null = null;

function render(
  r: RecordingRow,
  showInspect = false,
  over: Record<string, unknown> = {},
): HTMLElement {
  host = document.createElement("div");
  document.body.append(host);
  instance = mount(Row, {
    target: host,
    props: {
      row: r,
      onopen: noop,
      onreview: noop,
      onpin: noop,
      ondelete: noop,
      oninspect: noop,
      showInspect,
      ...over,
    },
  });
  return host;
}

afterEach(async () => {
  if (instance) await unmount(instance, { outro: false });
  host?.remove();
  instance = null;
  host = null;
});

describe("a row with nothing known", () => {
  it("still renders every column", () => {
    // The shape is the point. An imported file has a path and a size and
    // nothing else, and it has to sit in the same grid as a ranked game.
    const el = render(row());
    expect(el.querySelector(".vod-meta")).not.toBeNull();
    expect(el.querySelector(".vod-portrait")).not.toBeNull();
    expect(el.querySelectorAll(".vod-cell").length).toBeGreaterThanOrEqual(4);
    expect(el.querySelector(".vod-perks")).not.toBeNull();
    expect(el.querySelector(".vod-items")).not.toBeNull();
    expect(el.querySelector(".vod-versus")).not.toBeNull();
    expect(el.querySelector(".vod-actions")).not.toBeNull();
  });

  it("marks the missing values rather than leaving gaps", () => {
    // Queue, length, KDA, CS and rank are all unknown here.
    expect(render(row()).querySelectorAll(".vod-missing").length).toBeGreaterThanOrEqual(5);
  });

  it("says Unknown for the role, in words", () => {
    const el = render(row());
    expect(el.textContent).toContain("Unknown");
  });

  it("says there is no matchup rather than drawing empty boxes", () => {
    // A row of blank boxes beside a "vs" reads as art that failed to load,
    // which is a bug report waiting to happen.
    const el = render(row());
    expect(el.querySelector(".vod-versus")?.getAttribute("data-unknown")).toBe("true");
    expect(el.textContent).toContain("No matchup recorded");
  });

  it("carries no outcome, which is how undecided is said", () => {
    // Absent rather than a third word: a word is on every decided row, so no
    // word means undecided.
    const el = render(row());
    expect(el.querySelector(".vod-row")?.getAttribute("data-outcome")).toBe("unknown");
    expect(el.querySelector(".vod-outcome")).toBeNull();
  });
});

describe("the outcome", () => {
  it("says Win and marks the row", () => {
    const el = render(row({ win: true }));
    expect(el.querySelector(".vod-row")?.getAttribute("data-outcome")).toBe("win");
    expect(el.querySelector(".vod-outcome")?.textContent).toBe("Win");
  });

  it("says Loss and marks the row", () => {
    const el = render(row({ win: false }));
    expect(el.querySelector(".vod-row")?.getAttribute("data-outcome")).toBe("loss");
    expect(el.querySelector(".vod-outcome")?.textContent).toBe("Loss");
  });
});

describe("KDA", () => {
  it("shows all three, with deaths picked out", () => {
    const el = render(row({ kda_k: 7, kda_d: 2, kda_a: 11 }));
    const kda = el.querySelector(".vod-kda");
    expect(kda?.textContent?.replace(/\s+/g, " ")).toContain("7 / 2 / 11");
    expect(kda?.querySelector(".vod-deaths")?.textContent).toBe("2");
  });

  it("is all three or nothing", () => {
    // A partial KDA reads as a real one.
    const el = render(row({ kda_k: 7, kda_d: null, kda_a: 11 }));
    expect(el.querySelector(".vod-kda .vod-missing")).not.toBeNull();
  });
});

describe("the rank column", () => {
  it("shows the tier and division", () => {
    const el = render(row({ tier: "GOLD", division: "II" }));
    expect(el.textContent).toContain("Gold II");
  });

  it("shows no LP without a tier", () => {
    // A number with no scale is not a standing, and a row from before
    // migration 10 has neither.
    const el = render(row({ tier: null, division: null, lp_after: 42 }));
    expect(el.textContent).not.toContain("42 LP");
  });
});

describe("the actions", () => {
  it("reflects the pinned state on the button", () => {
    expect(
      render(row({ pinned: true }))
        .querySelector(".pin-btn")
        ?.getAttribute("aria-pressed"),
    ).toBe("true");
    expect(
      render(row({ pinned: false }))
        .querySelector(".pin-btn")
        ?.getAttribute("aria-pressed"),
    ).toBe("false");
  });

  it("hides the inspect button unless the build has the dev commands", () => {
    expect(
      render(row(), false).querySelector('[aria-label="Inspect in the dev portal"]'),
    ).toBeNull();
    expect(
      render(row(), true).querySelector('[aria-label="Inspect in the dev portal"]'),
    ).not.toBeNull();
  });
});

describe("a row with a full scoreboard", () => {
  const board = {
    players: [
      {
        champion: "Ahri",
        team: "ORDER",
        position: "Middle",
        is_us: true,
        level: 18,
        kills: 9,
        deaths: 3,
        assists: 7,
        cs: 240,
        items: [3020, 6655, 3157],
        spells: ["Flash", "Ignite"],
      },
      {
        champion: "Zed",
        team: "CHAOS",
        position: "Middle",
        level: 17,
        kills: 4,
        deaths: 6,
        assists: 2,
        cs: 198,
        items: [3006, 6693],
        spells: [],
      },
    ],
    our_runes: {
      keystone_id: 8112,
      keystone: "Electrocute",
      primary_tree_id: 8100,
      secondary_tree_id: 8300,
    },
  };

  const withBoard = (over: Partial<RecordingRow> = {}) =>
    row({ scoreboard_json: JSON.stringify(board), ...over });

  it("draws the lane opponent rather than the whole lobby", () => {
    // Ten champions told you who was in the game; one tells you who you
    // actually played against, which is what a person is reconstructing.
    const el = render(withBoard());
    const versus = el.querySelector(".vod-versus");
    expect(versus?.getAttribute("data-unknown")).toBeNull();
    expect(versus?.textContent).toContain("4");
    expect(versus?.textContent).toContain("198 cs");
  });

  it("renders every inventory slot, filled or not", () => {
    // **Empty slots are rendered, not skipped.** A build with three items is a
    // different thing from a game with no scoreboard, and a row that shrank to
    // fit would say neither.
    const el = render(withBoard());
    const ours = el.querySelector(".vod-items");
    expect(ours?.querySelectorAll(".vod-slot")).toHaveLength(7);
    expect(ours?.querySelectorAll(".vod-slot-empty")).toHaveLength(4);
  });

  it("renders two spells and two runes, in that order", () => {
    // Order is load-bearing: the grid fills by column, so these four land as
    // spell 1, spell 2 | keystone, secondary tree.
    const perks = render(withBoard()).querySelector(".vod-perks");
    const slots = perks?.querySelectorAll(".vod-slot") ?? [];
    expect(slots).toHaveLength(4);
    expect(slots[0].getAttribute("title")).toBe("Flash");
    expect(slots[1].getAttribute("title")).toBe("Ignite");
    expect(slots[2].getAttribute("title")).toBe("Electrocute");
    expect(slots[3].getAttribute("title")).toBe("Secondary tree");
  });

  it("pads the perks when there is no rune page", () => {
    const el = render(
      row({
        scoreboard_json: JSON.stringify({ players: board.players }),
      }),
    );
    const perks = el.querySelector(".vod-perks");
    expect(perks?.querySelectorAll(".vod-slot")).toHaveLength(4);
    expect(perks?.querySelectorAll(".vod-slot-empty")).toHaveLength(2);
  });

  it("uses spell ids when the board was rebuilt from match history", () => {
    // Names when captured live, ids when rebuilt. Both find the art; neither
    // is converted into the other.
    const el = render(
      row({
        scoreboard_json: JSON.stringify({
          players: [{ ...board.players[0], spells: [], spell_ids: [4, 14] }],
        }),
      }),
    );
    const slots = el.querySelector(".vod-perks")?.querySelectorAll(".vod-slot") ?? [];
    expect(slots[0].getAttribute("title")).toBe("Spell 4");
    expect(slots[1].getAttribute("title")).toBe("Spell 14");
  });

  it("names the opponent in the accessible name", () => {
    expect(render(withBoard()).querySelector(".vod-row")?.getAttribute("aria-label")).toContain(
      "Zed",
    );
  });
});

describe("the actions do not open the recording", () => {
  /**
   * `library.ts` did this by checking `closest(".vod-actions")` in a delegated
   * handler on the grid. Without it, pinning or deleting also opens the VOD,
   * which for delete means opening the thing you just destroyed.
   *
   * Tested through `Row` rather than `RowActions`: Svelte delegates these
   * events to the mount root, so `stopPropagation` in the child is only
   * meaningful against the parent handler it is actually guarding, which is
   * the article's.
   */
  it("pinning does not open it", async () => {
    const onopen = vi.fn();
    const el = render(row(), false, { onopen });
    el.querySelector<HTMLButtonElement>(".pin-btn")?.click();
    await Promise.resolve();
    expect(onopen).not.toHaveBeenCalled();
  });

  it("arming a delete does not open it", async () => {
    const onopen = vi.fn();
    const el = render(row(), false, { onopen });
    el.querySelector<HTMLButtonElement>('[aria-label="Delete recording"]')?.click();
    await Promise.resolve();
    expect(onopen).not.toHaveBeenCalled();
  });

  it("clicking the row itself does open it", async () => {
    // The other half: without this the first two tests would pass on a row
    // that is not clickable at all.
    const onopen = vi.fn();
    const el = render(row(), false, { onopen });
    el.querySelector<HTMLElement>(".vod-champ")?.click();
    await Promise.resolve();
    expect(onopen).toHaveBeenCalledOnce();
  });
});

describe("untrusted input", () => {
  /**
   * **The reason WS4 replaced this template.** `db::reconcile` imports whatever
   * video file the user drops into the recordings folder, `vodTitle` falls back
   * to the filename, and `library.ts` built the row by concatenating HTML with
   * `escapeHtml` / `escapeAttr` applied by hand at each site. Svelte's default
   * interpolation is what replaces both, and this is the test that fails if an
   * `{@html}` is ever introduced here.
   */
  it("renders a filename that looks like markup as text", () => {
    const nasty = "<img src=x onerror=alert(1)>";
    const el = render(row({ path: `C:/vods/${nasty}.mp4` }));

    // Nothing was parsed as markup: no element came out of it, and the
    // champion cell holds the string itself.
    expect(el.querySelectorAll("img")).toHaveLength(0);
    expect(el.querySelector(".vod-champ")?.textContent).toBe(`${nasty}.mp4`);

    // Asserting on `innerHTML` here would be a trap worth naming: the same
    // string is also a `title` attribute, and serializing an attribute
    // escapes `&` and `"` but not `<`, so `innerHTML` legitimately contains
    // `<img` inside a quoted value. That is not markup and never becomes
    // markup; what matters is that it is an attribute Svelte *set*, rather
    // than characters concatenated into a template.
    expect(el.querySelector(".vod-champ")?.getAttribute("title")).toBe(`${nasty}.mp4`);
  });

  it("puts it in the accessible name as text too", () => {
    const nasty = '" onmouseover="alert(1)';
    const el = render(row({ path: `C:/vods/${nasty}.mp4` }));
    const article = el.querySelector(".vod-row");
    expect(article?.getAttribute("onmouseover")).toBeNull();
    expect(article?.getAttribute("aria-label")).toContain(nasty);
  });
});
