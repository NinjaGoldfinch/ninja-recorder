import { flushSync, mount, unmount } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";
import { resizeTo } from "../../../test-setup";
import type { MarkerRow, SampleRow } from "../../../types";
import type { MetricKey } from "../../timeline/graph";
import { viewingWindow } from "../../timeline/window";
import Timeline from "./Timeline.svelte";

/**
 * The timeline's five states and its playhead.
 *
 * Clustering is measured in pixels and jsdom reports every element as zero
 * wide, so glyph *placement* is not testable here and is covered directly in
 * `clusters.test.ts`. What this covers is everything the width does not gate:
 * which of the graph's five states shows, what the picker does in each, the
 * ruler, and the playhead.
 */

const marker = (over: Partial<MarkerRow> = {}): MarkerRow =>
  ({
    id: 1,
    recording_id: 1,
    game_time_s: 0,
    video_time_s: 0,
    kind: "kill",
    payload_json: "{}",
    ...over,
  }) as MarkerRow;

const sample = (over: Partial<SampleRow> = {}): SampleRow =>
  ({
    id: 1,
    recording_id: 1,
    game_time_s: 0,
    video_time_s: 0,
    our_team: "ORDER",
    gold_diff: null,
    kill_diff: null,
    cs_diff: null,
    our_gold: null,
    our_level: null,
    ...over,
  }) as SampleRow;

let host: HTMLElement | null = null;
let instance: Record<string, unknown> | null = null;

function render(over: Record<string, unknown> = {}) {
  host = document.createElement("div");
  document.body.append(host);
  instance = mount(Timeline, {
    target: host,
    props: {
      markers: [] as MarkerRow[],
      samples: [] as SampleRow[],
      metric: "gold_diff" as MetricKey,
      window: viewingWindow(0, 600, 600),
      currentTimeS: 0,
      onmetric: () => {},
      onseek: () => {},
      onscrub: () => {},
      onscrubstart: () => {},
      ...over,
    },
  });
  return host;
}

const picker = (el: HTMLElement) => el.querySelector<HTMLSelectElement>(".timeline-metric-select");

afterEach(async () => {
  if (instance) await unmount(instance, { outro: false });
  host?.remove();
  instance = null;
  host = null;
});

describe("the graph's states", () => {
  it("hides the picker entirely when there is no data at all", () => {
    const el = render({ samples: [] });
    expect(el.textContent).toContain("No metric data for this recording");
    expect(picker(el)?.hidden).toBe(true);
    expect(el.querySelector(".timeline-graph")).toBeNull();
  });

  it("disables the picker when the sign of every diff is unknowable", () => {
    // **The state that matters most.** Drawing the curve anyway would risk
    // telling someone they were ahead in a game they lost. The picker is
    // disabled rather than hidden: the data exists, it just cannot be read.
    const el = render({ samples: [sample({ our_team: null, gold_diff: 500 })] });
    expect(el.textContent).toContain("Team side unknown");
    expect(picker(el)?.hidden).toBe(false);
    expect(picker(el)?.disabled).toBe(true);
  });

  it("leaves the picker usable when the user turned the graph off", () => {
    // A choice, not an absence.
    const el = render({
      metric: "none",
      samples: [
        sample({ gold_diff: 1, video_time_s: 1 }),
        sample({ gold_diff: 2, video_time_s: 2 }),
      ],
    });
    expect(el.querySelector(".timeline-graph")).toBeNull();
    expect(picker(el)?.disabled).toBe(false);
  });

  it("gives gold its own message", () => {
    const samples = [sample({ video_time_s: 1 }), sample({ video_time_s: 2 })];
    expect(render({ samples, metric: "gold_diff" }).textContent).toContain(
      "No gold data for this recording",
    );
  });

  it("draws the curve, the baseline and both filled halves", () => {
    const el = render({
      samples: [
        sample({ gold_diff: -1000, video_time_s: 0 }),
        sample({ gold_diff: 2000, video_time_s: 600 }),
      ],
    });
    const svg = el.querySelector(".timeline-graph");
    expect(svg).not.toBeNull();
    expect(svg?.querySelectorAll(".tl-area")).toHaveLength(2);
    expect(svg?.querySelector(".tl-baseline")).not.toBeNull();
    expect(svg?.querySelector(".tl-line")?.getAttribute("d")).toContain("M");
    expect(el.textContent).toContain("peak +2.0k");
  });

  it("reports a metric change rather than applying it itself", () => {
    // The store owns the choice, so a second view of the same recording
    // cannot disagree with this one.
    const onmetric = vi.fn();
    const el = render({
      samples: [
        sample({ gold_diff: 1, video_time_s: 1 }),
        sample({ gold_diff: 2, video_time_s: 2 }),
      ],
      onmetric,
    });
    const select = picker(el);
    if (!select) throw new Error("no picker");
    select.value = "cs_diff";
    // Bubbling, because Svelte delegates these to the mount root.
    select.dispatchEvent(new Event("change", { bubbles: true }));
    expect(onmetric).toHaveBeenCalledWith("cs_diff");
  });
});

describe("the ruler", () => {
  it("labels from zero, which is where the window starts", () => {
    // The ruler reads 0:00 where the player starts, so the skipped loading
    // screen is not a stretch of timeline with nothing in it.
    const labels = render().querySelectorAll(".ruler-label");
    expect(labels.length).toBeGreaterThan(1);
    expect(labels[0].textContent?.trim()).toBe("0:00");
  });

  it("renders nothing to measure against for an empty window", () => {
    const el = render({ window: viewingWindow(0, null, 0) });
    expect(el.querySelectorAll(".ruler-label")).toHaveLength(0);
  });
});

describe("the playhead", () => {
  it("sits at the fraction of the window the video is at", () => {
    const el = render({ window: viewingWindow(0, 100, 100), currentTimeS: 50 });
    // Parsed rather than compared as a string: jsdom normalises "50.000%".
    const left = el.querySelector<HTMLElement>(".timeline-playhead")?.style.left ?? "";
    expect(Number.parseFloat(left)).toBeCloseTo(50);
  });

  it("is pinned at zero before anything has loaded", () => {
    const el = render({ window: viewingWindow(0, null, 0), currentTimeS: 0 });
    const left = el.querySelector<HTMLElement>(".timeline-playhead")?.style.left ?? "";
    expect(Number.parseFloat(left)).toBe(0);
  });
});

describe("markers", () => {
  it("draws no glyphs when the body has no measured width", () => {
    // jsdom reports zero, and `clusterMarkers` correctly returns nothing at
    // zero width rather than piling every marker at the left edge.
    const el = render({ markers: [marker({ video_time_s: 10 })] });
    expect(el.querySelectorAll(".marker-glyph")).toHaveLength(0);
  });
});

describe("the cluster tooltip", () => {
  /**
   * jsdom lays nothing out, so every measurement the placement reads is zero
   * unless it is stubbed. These stub only what the code actually reads, which
   * is the width of the body, the tooltip's own width, and whether its content
   * overflows.
   */
  /** The width the clustering measures in, said out loud. Without it there
   *  are no glyphs at all, which is jsdom telling the truth about layout. */
  function widen(el: HTMLElement, width = 400) {
    const bodyEl = el.querySelector<HTMLElement>(".timeline-body");
    if (bodyEl) resizeTo(bodyEl, width);
    flushSync();
  }

  function hover(el: HTMLElement) {
    const glyph = el.querySelector<HTMLElement>(".marker-glyph");
    if (!glyph) throw new Error("no marker glyph");
    glyph.dispatchEvent(new MouseEvent("mouseenter", { bubbles: false }));
    return glyph;
  }

  /**
   * Hovers, measures, hovers again.
   *
   * The tooltip does not exist until something is hovered, so its dimensions
   * cannot be stubbed before the first hover, and placement reads them. The
   * second hover is what places it with the measurements in hand.
   */
  async function openTooltip(el: HTMLElement, { width = 100, overflows = false } = {}) {
    hover(el);
    await Promise.resolve();
    const tooltip = el.querySelector<HTMLElement>(".timeline-tooltip");
    if (tooltip) {
      Object.defineProperty(tooltip, "offsetWidth", { value: width, configurable: true });
      Object.defineProperty(tooltip, "scrollHeight", {
        value: overflows ? 500 : 50,
        configurable: true,
      });
      Object.defineProperty(tooltip, "clientHeight", { value: 50, configurable: true });
    }
    hover(el);
    await Promise.resolve();
    return el.querySelector<HTMLElement>(".timeline-tooltip");
  }

  it("shows the markers in the cluster it was opened on", async () => {
    const el = render({ markers: [marker({ id: 1, video_time_s: 60, kind: "kill" })] });
    widen(el);
    const tooltip = await openTooltip(el);
    expect(tooltip?.textContent).toContain("1:00");
  });

  it("stays inside the timeline at either end", async () => {
    const el = render({ markers: [marker({ id: 1, video_time_s: 0 })] });
    widen(el, 400);
    const tooltip = await openTooltip(el, { width: 100 });
    const left = Number.parseFloat(tooltip?.style.left ?? "0");
    // Half its own width in from the edge, never off it.
    expect(left).toBeGreaterThanOrEqual(50);
    expect(left).toBeLessThanOrEqual(350);
  });

  it("takes the pointer only when it actually overflows", async () => {
    // It overlaps the top of the track, so making it hoverable unconditionally
    // would put a dead strip over the glyphs underneath.
    const el = render({ markers: [marker({ id: 1, video_time_s: 60 })] });
    widen(el);
    expect((await openTooltip(el, { overflows: false }))?.dataset.scrollable).toBe("false");

    const scrollable = render({ markers: [marker({ id: 2, video_time_s: 60 })] });
    widen(scrollable);
    expect((await openTooltip(scrollable, { overflows: true }))?.dataset.scrollable).toBe("true");
  });

  it("survives a hover before the tooltip has been laid out", async () => {
    const el = render({ markers: [marker({ id: 1, video_time_s: 60 })] });
    // A width, so the glyph exists, but nothing stubbed on the tooltip: every
    // measurement it makes is zero, which is what a hover on the first frame
    // really sees.
    widen(el);
    hover(el);
    await Promise.resolve();
    expect(el.querySelector(".vod-timeline")).not.toBeNull();
  });

  it("seeks to the first marker in the cluster when the glyph is pressed", () => {
    const seeks: number[] = [];
    const el = render({
      markers: [marker({ id: 1, video_time_s: 90 })],
      onseek: (t: number) => seeks.push(t),
    });
    widen(el);
    el.querySelector<HTMLButtonElement>(".marker-glyph")?.click();
    expect(seeks).toEqual([90]);
  });

  it("names every marker in the cluster for a screen reader", () => {
    const el = render({ markers: [marker({ id: 1, video_time_s: 90, kind: "kill" })] });
    widen(el);
    const label = el.querySelector(".marker-glyph")?.getAttribute("aria-label");
    expect(label).toContain("1:30");
  });
});
