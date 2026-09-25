import { describe, expect, it } from "vitest";
import { bytes, clockTime, duration, MISSING, timestamp, workerState } from "./format";

/**
 * These were untested for as long as the portal was out of scope: WS4's plan
 * had it staying vanilla, so `vitest.config.ts` excluded `src/dev/**`. #72
 * reversed that.
 */

describe("bytes", () => {
  it("picks a unit by magnitude", () => {
    expect(bytes(512)).toBe("512 B");
    expect(bytes(2048)).toBe("2 KB");
    expect(bytes(5 * 1024 ** 2)).toBe("5.0 MB");
    expect(bytes(3 * 1024 ** 3)).toBe("3.00 GB");
  });

  it("uses more precision the larger the unit", () => {
    // A gigabyte figure is the one people compare against a disk, so it keeps
    // two places where a kilobyte figure keeps none.
    expect(bytes(1024 ** 3 + 512 * 1024 ** 2)).toBe("1.50 GB");
    expect(bytes(1536)).toBe("2 KB");
  });

  it("says nothing rather than zero for a missing value", () => {
    expect(bytes(null)).toBe(MISSING);
    expect(bytes(undefined)).toBe(MISSING);
    // Zero is a real answer and is not the same thing.
    expect(bytes(0)).toBe("0 B");
  });

  it("picks the unit off the magnitude, so a negative delta still reads right", () => {
    expect(bytes(-5 * 1024 ** 2)).toBe("-5.0 MB");
  });
});

describe("duration", () => {
  it("is m:ss with a padded seconds field", () => {
    expect(duration(0)).toBe("0:00");
    expect(duration(9)).toBe("0:09");
    expect(duration(75)).toBe("1:15");
    expect(duration(3600)).toBe("60:00");
  });

  it("rounds rather than truncating", () => {
    expect(duration(59.6)).toBe("1:00");
  });

  it("clamps a negative to zero", () => {
    // A duration derived from two clocks can go slightly negative while a
    // recording is starting, and "-0:01" reads as a bug in the panel.
    expect(duration(-5)).toBe("0:00");
  });

  it("says nothing for a missing value", () => {
    expect(duration(null)).toBe(MISSING);
    expect(duration(undefined)).toBe(MISSING);
  });
});

describe("clockTime", () => {
  it("is 24-hour, so it needs no am/pm to be read beside a filename", () => {
    // Asserted through the shape rather than an exact string: the locale is
    // the runner's, and the property that matters is the absence of a
    // meridiem.
    const at = clockTime(Date.UTC(2026, 0, 2, 13, 5, 9));
    expect(at).not.toMatch(/[ap]\.?m\.?/i);
    expect(at).toMatch(/\d{1,2}[:.]\d{2}[:.]\d{2}/);
  });
});

describe("timestamp", () => {
  it("carries the date as well as the time", () => {
    const at = timestamp(Date.UTC(2026, 0, 2, 13, 5, 9));
    expect(at.length).toBeGreaterThan(clockTime(Date.UTC(2026, 0, 2, 13, 5, 9)).length);
  });
});

describe("workerState", () => {
  it("tells a backend with no worker apart from a worker that is down", () => {
    expect(workerState(true)).toBe("up");
    expect(workerState(false)).toBe("not running");
    expect(workerState(null)).toContain("no worker");
  });

  it("is missing before the first poll", () => {
    expect(workerState(undefined)).toBe(MISSING);
  });
});
