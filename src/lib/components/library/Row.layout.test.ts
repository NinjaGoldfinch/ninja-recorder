import { flushSync, mount } from "svelte";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { page } from "vitest/browser";
import live from "../../../../fixtures/live-client/allgamedata-paired.json";
import { useLayoutFonts } from "../../../layout-fonts";
import type { RecordingRow, Scoreboard, ScoreboardPlayer } from "../../../types";
import "../../styles/app.css";
import Row from "./Row.svelte";

/**
 * The layout gate (#345): library rows, in Chromium, at every window width the
 * app allows.
 *
 * **Why this exists.** Rows drew past their card at both ends of the window
 * range (#342) with every other gate green, because every other frontend test
 * runs in jsdom, which does no layout. This one mounts the real `Row` under the
 * real stylesheet and measures, from the 960px minimum window to 2560px, and
 * fails if:
 *
 * 1. any element is drawn outside its row's box. Every descendant is checked,
 *    not a list of selectors, so a block added later is covered without
 *    touching this file;
 * 2. the page scrolls sideways;
 * 3. a number is cut short. Names may ellipsize; K/D/A, CS, rank and size
 *    may not;
 * 4. a column starts at a different x on different rows. The list is one grid
 *    and every row a subgrid of it (#341), and this is the property that buys.
 *
 * **Fonts.** Text width decides most of this, so the font stack is pinned;
 * see `src/layout-fonts.ts`.
 */

// The scrollbar WebView2 draws, which the page loses from a window's width.
const SCROLLBAR = 17;
const WIDTHS = Array.from({ length: (2560 - 960) / 16 + 1 }, (_, i) => 960 + i * 16);

// --- the rows ---------------------------------------------------------------

type LivePlayer = (typeof live.allPlayers)[number];
const POSITION: Record<string, string> = {
  TOP: "Top",
  JUNGLE: "Jungle",
  MIDDLE: "Middle",
  BOTTOM: "Bottom",
  UTILITY: "Support",
};

/** A player as the #346 writers store them, from the paired live capture. */
function stored(p: LivePlayer, us: string, roleItem?: number): ScoreboardPlayer {
  const items = [...p.items].sort((a, b) => a.slot - b.slot);
  return {
    champion: p.championName,
    team: p.team,
    is_us: p.championName === us,
    level: p.level,
    kills: p.scores.kills,
    deaths: p.scores.deaths,
    assists: p.scores.assists,
    cs: p.scores.creepScore,
    position: POSITION[p.position] ?? null,
    items: items.map((i) => i.itemID),
    trinket: items.find((i) => i.slot === 6)?.itemID ?? null,
    role_item: roleItem ?? null,
    spells: [
      p.summonerSpells.summonerSpellOne.displayName,
      p.summonerSpells.summonerSpellTwo.displayName,
    ],
    runes: {
      keystone_id: p.runes.keystone.id,
      keystone: p.runes.keystone.displayName,
      primary_tree_id: p.runes.primaryRuneTree.id,
      secondary_tree_id: p.runes.secondaryRuneTree.id,
    },
  };
}

function board(us: string, roleItems: Record<string, number> = {}): Scoreboard {
  return {
    our_team: "ORDER",
    players: live.allPlayers.map((p) => stored(p, us, roleItems[p.championName])),
  };
}

function row(id: number, over: Partial<RecordingRow> = {}): RecordingRow {
  return {
    id,
    path: `C:/vods/recording-${id}.mp4`,
    started_at: Date.now() - id * 86_400_000,
    duration_s: 1897,
    game_id: id,
    queue: 420,
    champion: null,
    role: null,
    win: null,
    kda_k: null,
    kda_d: null,
    kda_a: null,
    patch: "16.19.821.7343",
    pinned: false,
    size_bytes: 1_400_000_000,
    audio_tracks_json: null,
    game_mode: "CLASSIC",
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

/**
 * Every shape a row can take, and the widest version of each thing in it.
 * Adding a new kind of row here is how a new edge case joins the gate.
 */
const ROWS: { name: string; row: RecordingRow; inspect?: boolean; armed?: boolean }[] = [
  {
    name: "a full ranked game",
    row: row(1, {
      champion: "Viego",
      role: "Jungle",
      win: true,
      kda_k: 11,
      kda_d: 3,
      kda_a: 5,
      cs: 197,
      tier: "EMERALD",
      division: "III",
      lp_after: 36,
      scoreboard_json: JSON.stringify(board("Viego", { Viego: 1209, Qiyana: 1209 })),
    }),
  },
  {
    name: "a bot laner, with the boots box, pinned",
    row: row(2, {
      champion: "Kalista",
      role: "Bottom",
      win: false,
      pinned: true,
      kda_k: 7,
      kda_d: 4,
      kda_a: 8,
      cs: 244,
      tier: "GRANDMASTER",
      division: null,
      lp_after: 1284,
      scoreboard_json: JSON.stringify(board("Kalista", { Kalista: 3008, Twitch: 3006 })),
    }),
  },
  {
    name: "the widest numbers, with the dev inspector and a delete armed",
    inspect: true,
    armed: true,
    row: row(3, {
      champion: "Nunu & Willump",
      role: "Support",
      win: true,
      duration_s: 3725,
      kda_k: 28,
      kda_d: 14,
      kda_a: 37,
      cs: 1024,
      tier: "EMERALD",
      division: "III",
      lp_after: 100,
      size_bytes: 12_800_000_000,
      scoreboard_json: JSON.stringify(board("Viego")),
    }),
  },
  {
    name: "no rune page, no opponent, and a capture note",
    row: row(4, {
      queue: 2400,
      game_mode: "KIWI",
      champion: "Lux",
      win: false,
      duration_s: 1060,
      kda_k: 14,
      kda_d: 6,
      kda_a: 30,
      cs: 34,
      diagnostics_json: JSON.stringify({
        capture_problems: [{ kind: "sourceFailed", source: "game", reason: "refused" }],
      }),
      scoreboard_json: JSON.stringify({
        players: [
          {
            champion: "Lux",
            team: "ORDER",
            is_us: true,
            level: 18,
            kills: 14,
            deaths: 6,
            assists: 30,
            cs: 34,
            items: [3020, 6655, 4645, 3089],
            spells: ["Flash", "Snowball"],
          },
        ],
      }),
    }),
  },
  {
    name: "an imported file with nothing known and a long name",
    row: row(5, {
      path: "C:/vods/a-very-long-imported-recording-filename-that-goes-on-and-on-2026-09-02.mp4",
      duration_s: null,
      queue: 9999,
      game_mode: null,
      patch: null,
      size_bytes: 21_000_000,
    }),
  },
];

// --- the checks ---------------------------------------------------------------

function inside(el: Element, box: DOMRect): boolean {
  const e = el.getBoundingClientRect();
  if (e.width === 0 && e.height === 0) return true;
  return (
    e.left >= box.left - 0.5 &&
    e.right <= box.right + 0.5 &&
    e.top >= box.top - 0.5 &&
    e.bottom <= box.bottom + 0.5
  );
}

function describeEl(el: Element): string {
  const cls = [...el.classList].join(".");
  return cls ? `.${cls}` : el.tagName.toLowerCase();
}

function problemsAt(list: HTMLElement): string[] {
  const out: string[] = [];
  const rows = [...list.querySelectorAll<HTMLElement>(".vod-row")];

  for (const r of rows) {
    const name = ROWS.find((x) => String(x.row.id) === r.dataset.id)?.name ?? r.dataset.id;
    const box = r.getBoundingClientRect();
    for (const el of r.querySelectorAll("*")) {
      if (!inside(el, box)) out.push(`${name}: ${describeEl(el)} is drawn outside the row`);
    }
    for (const el of r.querySelectorAll<HTMLElement>(
      ".vod-kda, .vod-stats .vod-sub, .vod-rank, .vod-size",
    )) {
      if (el.scrollWidth > el.clientWidth + 1) {
        out.push(`${name}: "${el.textContent?.trim()}" is cut short`);
      }
    }
  }

  const root = document.documentElement;
  if (root.scrollWidth > root.clientWidth) out.push("the page scrolls sideways");

  for (const block of [
    ".vod-champion",
    ".vod-stats",
    ".vod-items",
    ".vod-versus",
    ".vod-slack",
    ".vod-actions",
  ]) {
    const xs = new Set(
      rows
        .map((r) => r.querySelector(`:scope > ${block}`)?.getBoundingClientRect().left)
        .filter((x): x is number => x !== undefined)
        .map(Math.round),
    );
    if (xs.size > 1) out.push(`${block} starts at ${[...xs].join(", ")} on different rows`);
  }
  return out;
}

/** `[960, 976, 992, 1600]` as `"960-992px, 1600px"`. */
function ranges(widths: number[]): string {
  const out: string[] = [];
  let start: number | undefined;
  let prev: number | undefined;
  for (const w of [...widths, Number.NaN]) {
    if (prev !== undefined && w === prev + 16) {
      prev = w;
      continue;
    }
    if (start !== undefined) out.push(start === prev ? `${start}px` : `${start}-${prev}px`);
    start = w;
    prev = w;
  }
  return out.join(", ");
}

// --- the test -------------------------------------------------------------------

describe("library rows at every window width", () => {
  let list: HTMLElement;
  let host: HTMLElement;

  beforeAll(async () => {
    await useLayoutFonts();

    // The same nesting the app renders, so the library's column cap applies.
    host = document.createElement("main");
    host.className = "container";
    host.innerHTML = `<section id="library-view"><div class="vod-list" role="list"></div></section>`;
    document.body.append(host);
    list = host.querySelector(".vod-list") as HTMLElement;

    const noop = () => {};
    for (const { row: r, inspect } of ROWS) {
      mount(Row, {
        target: list,
        props: {
          row: r,
          onopen: noop,
          onpin: noop,
          ondelete: noop,
          oninspect: noop,
          showInspect: !!inspect,
        },
      });
    }
    flushSync();
  });

  afterAll(() => host.remove());

  /** The delete button's armed state is the widest the actions get. It
   *  disarms after four seconds, so it is re-armed before each width. */
  function arm() {
    for (const { row: r, armed } of ROWS) {
      if (!armed) continue;
      const button = list.querySelector<HTMLButtonElement>(
        `.vod-row[data-id="${r.id}"] [aria-label="Delete recording"]`,
      );
      if (button && button.textContent !== "Delete?") button.click();
    }
    flushSync();
  }

  it("never draws anything outside a row, cuts a number short, or misaligns a column", async () => {
    const failures: string[] = [];
    const failing: number[] = [];
    for (const width of WIDTHS) {
      await page.viewport(width - SCROLLBAR, 900);
      arm();
      await new Promise((r) => requestAnimationFrame(() => r(null)));
      const problems = problemsAt(list);
      if (problems.length > 0) failing.push(width);
      for (const problem of problems) failures.push(`${width}px window: ${problem}`);
    }
    // Which widths fail, as ranges, then one line per failure: a regression
    // names its widths, its row and its element without anyone having to
    // reproduce it first.
    const summary = `failing windows: ${ranges(failing)}`;
    expect(failures, [summary, ...failures.slice(0, 40)].join("\n")).toEqual([]);
  });

  it("renders every row it was given", () => {
    // A row that failed to mount would pass every check above by absence.
    expect(list.querySelectorAll(".vod-row")).toHaveLength(ROWS.length);
    expect(
      list.querySelector('.vod-row[data-id="3"] [aria-label="Delete recording"]')?.textContent,
    ).toBe("Delete?");
  });
});
