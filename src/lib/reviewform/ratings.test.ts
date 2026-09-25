import { describe, expect, it } from "vitest";
import { GAME_CHOICES, LANE_CHOICES, MENTAL_CHOICES, toggle } from "./ratings";

describe("rating choices", () => {
  it("colours a good answer green, a middling one amber and a bad one red", () => {
    const tones = (choices: { value: string; tone: string }[]) =>
      Object.fromEntries(choices.map((c) => [c.value, c.tone]));
    expect(tones(GAME_CHOICES)).toEqual({ win: "good", loss: "bad" });
    expect(tones(LANE_CHOICES)).toEqual({ win: "good", neutral: "mid", loss: "bad" });
    expect(tones(MENTAL_CHOICES)).toEqual({ good: "good", neutral: "mid", bad: "bad" });
  });

  it("offers exactly the values the database accepts", () => {
    expect(GAME_CHOICES.map((c) => c.value)).toEqual(["win", "loss"]);
    expect(LANE_CHOICES.map((c) => c.value)).toEqual(["win", "neutral", "loss"]);
    expect(MENTAL_CHOICES.map((c) => c.value)).toEqual(["good", "neutral", "bad"]);
  });
});

describe("toggle", () => {
  it("chooses a value, switches between values, and unsets on a second click", () => {
    expect(toggle(null, "win")).toBe("win");
    expect(toggle("win", "loss")).toBe("loss");
    expect(toggle("loss", "loss")).toBeNull();
  });
});
