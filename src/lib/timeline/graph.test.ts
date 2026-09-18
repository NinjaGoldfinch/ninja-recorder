import { describe, expect, it } from "vitest";
import { downsample, MAX_RULER_LABELS, RULER_STEPS, rulerStep } from "./graph";

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
