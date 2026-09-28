import { flushSync, mount, unmount } from "svelte";
import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";
import { page } from "vitest/browser";
import { useLayoutFonts } from "../../../layout-fonts";
import "../../styles/app.css";

/**
 * The layout gate (#345): the review page, in Chromium, at every window shape
 * the app allows.
 *
 * **Why this exists.** The review was sized from hand-summed heights
 * (`--player-chrome`, `--rail-chrome`) and a fixed rail, which fit one window
 * shape. Every other shape left a band of nothing under the timeline or
 * beside the player, and a change to anything above the player moved the
 * sum without failing a test. This mounts the real `Review` under the real
 * stylesheet, below a stand-in app bar, and fails if at any size:
 *
 * 1. the page scrolls, or the ruler, the key hint or the rail's bottom edge
 *    is below the window;
 * 2. the player is not the recording's shape (it would be letterboxed), or
 *    is narrower than the 28rem floor;
 * 3. space is left over that something should have taken: between the
 *    player and the timeline at all, under the key hint while the timeline
 *    could still grow, or beside the player while the rail could still widen.
 *
 * Both with the rail open and in theatre mode.
 */

const call = vi.hoisted(() => vi.fn());
const client = vi.hoisted(() => ({
  open_game_for_recording: vi.fn(),
  get_game_review: vi.fn(),
  save_game_review: vi.fn(),
  add_note: vi.fn(),
  update_note: vi.fn(),
  delete_note: vi.fn(),
}));
vi.mock("../../../bridge", () => ({
  call,
  client,
  hasDevCommands: vi.fn().mockResolvedValue(false),
  assetUrl: (p: string) => p,
}));
vi.mock("../../../router", () => ({
  showView: vi.fn(),
  currentView: () => "review",
  onViewChange: vi.fn(),
  registerView: vi.fn(),
  initRouting: vi.fn(),
}));

const WIDTHS = [960, 1024, 1100, 1280, 1340, 1366, 1440, 1600, 1920, 2560, 3440];
const HEIGHTS = [640, 720, 768, 850, 900, 1000, 1080, 1200, 1440];

const REM = 16;
/** The timeline at its tallest: 5.5rem, plus the 5.5rem it may grow by. */
const TIMELINE_MAX = 11 * REM;
/** Under the key hint: the container's bottom padding, and a little for
 *  rounding and a hint that wrapped onto a second line. */
const SLACK = 2 * REM;

const row = {
  id: 1,
  path: "C:/vods/1.mp4",
  started_at: 0,
  duration_s: 1306,
  queue: 420,
  game_mode: "CLASSIC",
  champion: "Viego",
  win: true,
  kda_k: 19,
  kda_d: 2,
  kda_a: 2,
  cs: 188,
  tier: "EMERALD",
  division: "III",
  lp_after: 35,
  audio_tracks_json: null,
  scoreboard_json: null,
};

function box(host: HTMLElement, selector: string): DOMRect {
  const el = host.querySelector(selector);
  if (!el) throw new Error(`${selector} is not rendered`);
  return el.getBoundingClientRect();
}

function problemsAt(host: HTMLElement, theatre: boolean): string[] {
  const out: string[] = [];
  const root = document.documentElement;
  const bottom = window.innerHeight + 0.5;

  if (root.scrollHeight > root.clientHeight + 1) out.push("the page scrolls");
  if (root.scrollWidth > root.clientWidth + 1) out.push("the page scrolls sideways");

  const player = box(host, ".player-wrap");
  const video = box(host, "#review-video");
  const area = box(host, ".player-area");
  const timeline = box(host, ".timeline-body");
  const keys = box(host, ".review-keys");

  if (box(host, ".timeline-ruler").bottom > bottom) out.push("the ruler is below the window");
  if (keys.bottom > bottom) out.push("the key hint is below the window");

  if (Math.abs(player.width / player.height - 16 / 9) > 0.01) {
    out.push(`the player is ${player.width.toFixed(0)}×${player.height.toFixed(0)}, not 16/9`);
  }
  if (Math.abs(video.height - player.height) > 1 || Math.abs(video.width - player.width) > 1) {
    out.push("the video does not fill the player");
  }
  if (player.width < 28 * REM - 0.5) out.push(`the player is ${player.width.toFixed(0)}px wide`);
  // The player is the largest box of its shape the area holds, so it fills
  // the area one way or the other.
  if (area.width - player.width > 1.5 && area.height - player.height > 1.5) {
    out.push("the player fills its area in neither direction");
  }

  const head = box(host, ".timeline-head");
  if (head.top - player.bottom > REM) {
    out.push(
      `${(head.top - player.bottom).toFixed(0)}px is left between the player and the timeline`,
    );
  }

  const end = theatre ? box(host, ".review-body").bottom : box(host, ".review-rail").bottom;
  if (end > bottom) out.push(`the ${theatre ? "column" : "rail"} ends below the window`);
  if (end - keys.bottom > SLACK && timeline.height < TIMELINE_MAX - 0.5) {
    out.push(`${(end - keys.bottom).toFixed(0)}px is left under the key hint`);
  }

  if (!theatre) {
    const rail = box(host, ".review-rail");
    if (rail.width < 22 * REM - 0.5) out.push(`the rail is ${rail.width.toFixed(0)}px wide`);
    if (area.width - player.width > 1.5 && rail.width < 30 * REM - 0.5) {
      out.push("space is left beside the player while the rail could still widen");
    }
  }
  return out;
}

describe("the review page at every window shape", () => {
  let appRoot: HTMLElement;
  let host: HTMLElement;
  let instance: Record<string, unknown>;

  beforeAll(async () => {
    await useLayoutFonts();
    call.mockResolvedValue([]);
    client.open_game_for_recording.mockResolvedValue(70);
    client.get_game_review.mockResolvedValue({
      game: {
        id: 70,
        recording_id: 1,
        started_at: 0,
        ended_at: null,
        block_id: null,
        champion: "Viego",
        matchup: "Shaco",
        result: "win",
        recording_offset_ms: null,
      },
      review: null,
      death_markers: null,
      objectives: [],
      takeaways: [],
      notes: [],
    });

    // The DOM `index.html` and `App.svelte` build: everything is rendered
    // into `#app-root`, never straight into `<body>`. The review's height
    // comes down a flex chain from `<body>`, so a harness that left the
    // wrapper out passed while the app drew a zero-height player.
    appRoot = document.createElement("div");
    appRoot.id = "app-root";
    // The app bar's real height: a 2.5rem button row in its padding.
    const appBar = document.createElement("header");
    appBar.className = "app-bar";
    appBar.innerHTML = `<div class="app-bar-inner"><div style="height:2.5rem"></div></div>`;
    host = document.createElement("main");
    host.className = "container";
    host.innerHTML = `<section id="review-view" class="review-view"></section>`;
    appRoot.append(appBar, host);
    document.body.append(appRoot);

    const store = await import("../../stores/review.svelte");
    await store.openRecording(row as never);
    store.setDuration(row.duration_s);
    const Review = (await import("./Review.svelte")).default;
    instance = mount(Review, { target: host.querySelector("section") as HTMLElement });
    flushSync();
    await new Promise((r) => setTimeout(r, 50));
  });

  afterAll(() => {
    unmount(instance);
    appRoot.remove();
  });

  async function sweep(theatre: boolean): Promise<string[]> {
    const failures: string[] = [];
    for (const width of WIDTHS) {
      for (const height of HEIGHTS) {
        await page.viewport(width, height);
        await new Promise((r) => requestAnimationFrame(() => r(null)));
        for (const problem of problemsAt(host, theatre)) {
          failures.push(`${width}×${height}: ${problem}`);
        }
      }
    }
    return failures;
  }

  it("fits the window with the rail open", async () => {
    expect(host.querySelector(".review-rail")?.hasAttribute("hidden")).toBe(false);
    const failures = await sweep(false);
    expect(failures, failures.slice(0, 40).join("\n")).toEqual([]);
  });

  it("fits the window in theatre mode", async () => {
    (host.querySelector(".rail-toggle") as HTMLButtonElement).click();
    flushSync();
    expect(host.querySelector(".review-rail")?.hasAttribute("hidden")).toBe(true);
    const failures = await sweep(true);
    (host.querySelector(".rail-toggle") as HTMLButtonElement).click();
    flushSync();
    expect(failures, failures.slice(0, 40).join("\n")).toEqual([]);
  });
});
