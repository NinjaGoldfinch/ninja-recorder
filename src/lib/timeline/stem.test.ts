import { describe, expect, it } from "vitest";
import { SYNC_HARD, SYNC_NUDGE, stemCorrection } from "./stem";

/**
 * Two independent media elements drift. The thresholds decide whether that is
 * eased away or cut, and getting them the wrong way round is audible: a seek
 * where a nudge would do is a click every few seconds.
 */

describe("stemCorrection", () => {
  it("leaves an aligned stem alone", () => {
    expect(stemCorrection(0, 1)).toEqual({ seek: false, playbackRate: 1 });
  });

  it("does nothing for drift under the nudge threshold", () => {
    expect(stemCorrection(SYNC_NUDGE - 0.001, 1).seek).toBe(false);
    expect(stemCorrection(SYNC_NUDGE - 0.001, 1).playbackRate).toBe(1);
  });

  it("slows a stem that is ahead", () => {
    const c = stemCorrection(0.1, 1);
    expect(c.seek).toBe(false);
    expect(c.playbackRate).toBeLessThan(1);
  });

  it("speeds up a stem that is behind", () => {
    const c = stemCorrection(-0.1, 1);
    expect(c.seek).toBe(false);
    expect(c.playbackRate).toBeGreaterThan(1);
  });

  it("seeks rather than nudging past the hard threshold", () => {
    // A rate nudge big enough to close this gap would be audible as a pitch
    // shift, so the click of a seek is the lesser of the two.
    expect(stemCorrection(SYNC_HARD + 0.01, 1).seek).toBe(true);
    expect(stemCorrection(-(SYNC_HARD + 0.01), 1).seek).toBe(true);
  });

  it("restores the video's own rate when it seeks", () => {
    // A seek lands the stem exactly where the video is, and any easing rate
    // left over would immediately pull it off again.
    expect(stemCorrection(1, 1.5)).toEqual({ seek: true, playbackRate: 1.5 });
  });

  it("eases relative to the video's rate, not to 1x", () => {
    // Playing at 2x and nudging toward 0.98x would be a lurch, not a nudge.
    const c = stemCorrection(0.1, 2);
    expect(c.playbackRate).toBeCloseTo(2 * 0.98);
  });

  it("is symmetric about zero drift", () => {
    expect(stemCorrection(0.1, 1).playbackRate).toBeLessThan(1);
    expect(stemCorrection(-0.1, 1).playbackRate).toBeGreaterThan(1);
    expect(stemCorrection(SYNC_NUDGE, 1).playbackRate).toBe(1);
    expect(stemCorrection(-SYNC_NUDGE, 1).playbackRate).toBe(1);
  });
});
