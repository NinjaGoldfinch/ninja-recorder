import { describe, expect, it } from "vitest";
import type { SampleRow } from "../../types";
import {
  downsample,
  formatSigned,
  formatSignedGold,
  graphView,
  MAX_RULER_LABELS,
  metricValue,
  RULER_STEPS,
  rulerStep,
} from "./graph";
import { viewingWindow, windowFraction } from "./window";

describe("downsample", () => {
  it("leaves a series already short enough alone", () => {
    const points = [{ v: 1 }, { v: 2 }];
    expect(downsample(points, 10)).toBe(points);
  });

  it("buckets down to at most the target", () => {
    const points = Array.from({ length: 100 }, (_, i) => ({ v: i }));
    expect(downsample(points, 10)).toHaveLength(10);
  });

  it("keeps the largest magnitude in each bucket, not the largest value", () => {
    // The whole reason this is max-abs. A bucket holding a deep trough is a
    // bucket where the interesting number is negative, and plain max would
    // report the shallow positive beside it, turning a game you were losing
    // into a flat line.
    const points = [{ v: 1 }, { v: -50 }, { v: 2 }, { v: 3 }];
    expect(downsample(points, 2)).toEqual([{ v: -50 }, { v: 3 }]);
  });

  it("returns nothing for a target of zero or less", () => {
    expect(downsample([{ v: 1 }], 0)).toEqual([]);
    expect(downsample([{ v: 1 }], -1)).toEqual([]);
  });

  it("keeps the extremes of a signed series", () => {
    const points = Array.from({ length: 60 }, (_, i) => ({ v: Math.sin(i / 3) * 100 }));
    const out = downsample(points, 6);
    const peak = Math.max(...points.map((p) => Math.abs(p.v)));
    expect(Math.max(...out.map((p) => Math.abs(p.v)))).toBeCloseTo(peak);
  });
});

describe("rulerStep", () => {
  it("picks the coarsest step that stays under the label cap", () => {
    // 10 minutes at 15s steps would be 40 labels; 60s gives 10.
    expect(rulerStep(600)).toBe(60);
  });

  it("uses the finest step for a short window", () => {
    expect(rulerStep(60)).toBe(RULER_STEPS[0]);
  });

  it("never returns more labels than the cap, across realistic spans", () => {
    for (const span of [30, 120, 600, 1800, 3600, 7200]) {
      expect(span / rulerStep(span)).toBeLessThanOrEqual(MAX_RULER_LABELS);
    }
  });

  it("falls back to the coarsest step rather than returning nothing", () => {
    // A four-hour recording exceeds every candidate. Crowded ticks beat no
    // ticks, and `undefined` here would be a `NaN` gap in the CSS.
    const absurd = 60 * 60 * 24;
    expect(rulerStep(absurd)).toBe(RULER_STEPS[RULER_STEPS.length - 1]);
  });
});

// --- The advantage curve ---------------------------------------------------

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

const win = viewingWindow(0, 100, 100);
const view = (samples: SampleRow[], metric: Parameters<typeof graphView>[1] = "gold_diff") =>
  graphView(samples, metric, win, windowFraction);

describe("formatSigned", () => {
  it("keeps the sign visible on a positive number", () => {
    expect(formatSigned(12.4)).toBe("+12");
    expect(formatSigned(-12.4)).toBe("-12");
    expect(formatSigned(0)).toBe("0");
  });
});

describe("formatSignedGold", () => {
  it("abbreviates thousands", () => {
    expect(formatSignedGold(2500)).toBe("+2.5k");
    expect(formatSignedGold(-2500)).toBe("-2.5k");
  });

  it("leaves small numbers alone", () => {
    expect(formatSignedGold(430)).toBe("+430");
    expect(formatSignedGold(0)).toBe("0");
  });
});

describe("metricValue", () => {
  it("reads the column the metric names", () => {
    const s = sample({ gold_diff: 1, kill_diff: 2, cs_diff: 3 });
    expect(metricValue(s, "gold_diff")).toBe(1);
    expect(metricValue(s, "kill_diff")).toBe(2);
    expect(metricValue(s, "cs_diff")).toBe(3);
    expect(metricValue(s, "none")).toBeNull();
  });
});

describe("graphView", () => {
  it("says there is no data at all when there are no samples", () => {
    const v = view([]);
    expect(v.kind).toBe("noSamples");
    expect(v.picker).toBe("hidden");
  });

  it("refuses to draw a curve whose sign is unknowable", () => {
    // **The distinction that matters most.** A recording where we were never
    // matched in `allPlayers` has samples but no side, so every diff's sign is
    // a guess. Drawing it would risk telling someone they were ahead in a game
    // they lost.
    const v = view([sample({ our_team: null, gold_diff: 500, video_time_s: 1 })]);
    expect(v.kind).toBe("noSide");
    expect(v.picker).toBe("disabled");
    expect(v.summary).toContain("Team side unknown");
  });

  it("hides the curve when the user chose to", () => {
    const v = view([sample({ gold_diff: 1 })], "none");
    expect(v.kind).toBe("noMetric");
    // The picker stays usable: this is a choice, not an absence.
    expect(v.picker).toBe("enabled");
    expect(v.summary).toBe("");
  });

  it("gives gold its own message, because its absence has its own cause", () => {
    // Gold comes from the post-game timeline, so a custom or practice game
    // never has one. "Not enough data to plot" would read as a bug.
    const rows = [sample({ video_time_s: 1 }), sample({ video_time_s: 2 })];
    expect(view(rows, "gold_diff").summary).toBe("No gold data for this recording");
    expect(view(rows, "kill_diff").summary).toBe("Not enough data to plot");
  });

  it("needs two points to draw a line", () => {
    expect(view([sample({ gold_diff: 1, video_time_s: 1 })]).kind).toBe("tooSparse");
  });

  it("draws a curve and an area that closes on the baseline", () => {
    const v = view([
      sample({ gold_diff: -1000, video_time_s: 0 }),
      sample({ gold_diff: 1000, video_time_s: 100 }),
    ]);
    expect(v.kind).toBe("curve");
    if (v.kind !== "curve") return;

    expect(v.line.startsWith("M")).toBe(true);
    // The area returns to y=50, the zero line, at both ends. Filling to the
    // bottom edge instead would lose the ahead/behind split that is the whole
    // point of the shape.
    expect(v.area.startsWith("M0.00 50")).toBe(true);
    expect(v.area.endsWith("50 Z")).toBe(true);
  });

  it("is symmetric about zero, not min-to-max", () => {
    // A min-to-max scale floats the zero crossing, so a game spent entirely
    // behind renders as a line through the middle and reads as "even".
    const behind = view([
      sample({ gold_diff: -500, video_time_s: 0 }),
      sample({ gold_diff: -1000, video_time_s: 100 }),
    ]);
    if (behind.kind !== "curve") throw new Error("expected a curve");

    // Every y is below the baseline, because every value is negative.
    const ys = [...behind.line.matchAll(/[ML][\d.]+ ([\d.]+)/g)].map((m) => Number(m[1]));
    expect(ys.every((y) => y > 50)).toBe(true);
  });

  it("does not divide by zero on a perfectly even game", () => {
    const even = view([
      sample({ gold_diff: 0, video_time_s: 0 }),
      sample({ gold_diff: 0, video_time_s: 100 }),
    ]);
    if (even.kind !== "curve") throw new Error("expected a curve");
    expect(even.line).not.toContain("NaN");
  });

  it("summarises the end and the peak", () => {
    const v = view([
      sample({ gold_diff: 3000, video_time_s: 0 }),
      sample({ gold_diff: 500, video_time_s: 100 }),
    ]);
    expect(v.summary).toContain("+500 at end");
    expect(v.summary).toContain("peak +3.0k");
  });

  it("reports the largest swing either way as the peak", () => {
    // Max-abs, like the downsampling: the biggest thing that happened is the
    // deficit, and reporting the positive would describe a different game.
    const v = view([
      sample({ gold_diff: 200, video_time_s: 0 }),
      sample({ gold_diff: -4000, video_time_s: 50 }),
      sample({ gold_diff: 100, video_time_s: 100 }),
    ]);
    expect(v.summary).toContain("peak -4.0k");
  });
});
