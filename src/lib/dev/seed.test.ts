import { describe, expect, it } from "vitest";
import { BASE, PRESETS, type SeedForm, toForm, toSpec } from "./seed";

describe("presets", () => {
  it("have unique ids", () => {
    expect(new Set(PRESETS.map((p) => p.id)).size).toBe(PRESETS.length);
  });

  it("all say what they are for", () => {
    for (const preset of PRESETS) expect(preset.what.length).toBeGreaterThan(40);
  });

  it("produce a playable VOD from exactly one preset", () => {
    // Review is the only one that copies a real mp4; the rest write sparse
    // files that no demuxer will open.
    expect(PRESETS.filter((p) => p.spec.use_sample_mp4).map((p) => p.id)).toEqual(["review"]);
  });

  it("keep every duration range the right way round", () => {
    for (const { id, spec } of PRESETS) {
      expect(spec.duration_min_s, id).toBeLessThanOrEqual(spec.duration_max_s);
      expect(spec.markers_min, id).toBeLessThanOrEqual(spec.markers_max);
    }
  });

  it("do not share a random seed, so two presets never build the same library", () => {
    const seeds = PRESETS.map((p) => p.spec.seed);
    expect(new Set(seeds).size).toBe(seeds.length);
  });
});

describe("toSpec", () => {
  const fallback = BASE;

  it("round-trips a full form", () => {
    expect(toSpec(toForm(BASE), fallback)).toEqual(BASE);
  });

  it("takes an edited number", () => {
    const form: SeedForm = { ...toForm(BASE), count: 3 };
    expect(toSpec(form, fallback).count).toBe(3);
  });

  it("falls back to the preset for a cleared field rather than sending null", () => {
    const form: SeedForm = { ...toForm(BASE), count: null };
    expect(toSpec(form, fallback).count).toBe(BASE.count);
  });

  it("falls back for NaN too", () => {
    const form: SeedForm = { ...toForm(BASE), spread_days: Number.NaN };
    expect(toSpec(form, fallback).spread_days).toBe(BASE.spread_days);
  });

  it("keeps a deliberate zero, which is a meaningful value", () => {
    // `pinned_every: 0` pins nothing and `spread_days: 0` dates everything
    // today. Neither is "empty".
    const form: SeedForm = { ...toForm(BASE), pinned_every: 0, spread_days: 0 };
    const spec = toSpec(form, fallback);
    expect(spec.pinned_every).toBe(0);
    expect(spec.spread_days).toBe(0);
  });

  it("carries booleans through, including false", () => {
    const form: SeedForm = { ...toForm(BASE), samples: false, messy: true };
    const spec = toSpec(form, fallback);
    expect(spec.samples).toBe(false);
    expect(spec.messy).toBe(true);
  });

  it("uses the preset it was given, not BASE", () => {
    const review = PRESETS.find((p) => p.id === "review")!.spec;
    const form: SeedForm = { ...toForm(review), count: null };
    expect(toSpec(form, review).count).toBe(review.count);
  });
});
