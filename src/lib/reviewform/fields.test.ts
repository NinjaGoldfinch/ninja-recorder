import { describe, expect, it } from "vitest";
import { formatClock, parseClock, parseCount } from "./fields";

describe("clear time", () => {
  it("reads m:ss and mm:ss", () => {
    expect(parseClock("2:58")).toEqual({ ok: true, value: 178_000 });
    expect(parseClock(" 12:05 ")).toEqual({ ok: true, value: 725_000 });
  });

  it("treats a blank box as not entered, not as zero", () => {
    expect(parseClock("")).toEqual({ ok: true, value: null });
    expect(parseClock("   ")).toEqual({ ok: true, value: null });
  });

  it("refuses anything that is not a clock", () => {
    for (const text of ["178", "2:5", "2:60", "a:bc", "-1:00", "2:58.5", "123:00"]) {
      expect(parseClock(text), text).toEqual({ ok: false });
    }
  });

  it("formats what it parses", () => {
    expect(formatClock(178_000)).toBe("2:58");
    expect(formatClock(60_000)).toBe("1:00");
    expect(formatClock(null)).toBe("");
    const parsed = parseClock(formatClock(725_000));
    expect(parsed).toEqual({ ok: true, value: 725_000 });
  });
});

describe("counts", () => {
  it("reads whole numbers, including zero", () => {
    expect(parseCount("0")).toEqual({ ok: true, value: 0 });
    expect(parseCount(" 7 ")).toEqual({ ok: true, value: 7 });
  });

  it("treats a blank box as not entered", () => {
    expect(parseCount("")).toEqual({ ok: true, value: null });
  });

  it("refuses negatives, fractions and words", () => {
    for (const text of ["-1", "1.5", "two", "1e2"]) {
      expect(parseCount(text), text).toEqual({ ok: false });
    }
  });
});
