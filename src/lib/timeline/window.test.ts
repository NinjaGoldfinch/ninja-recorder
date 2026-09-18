import { describe, expect, it } from "vitest";
import type { SampleRow } from "../../types";
import {
  clamp,
  displayTime,
  LEAD_IN_S,
  MAX_TAIL_CLIP_S,
  MIN_SKIP_S,
  measureGameEnd,
  measureGameStart,
  viewingWindow,
  windowFraction,
} from "./window";

/**
 * The viewing window is what every seek is clamped to and what the ruler
 * reads 0:00 at, so getting it wrong shows up as a player that opens on the
 * wrong frame or a timeline whose positions mean nothing. None of it was
 * testable before WS4.2 pulled it out of `review.ts`, where it read module
 * state and a live `<video>`.
 */

function sample(game_time_s: number, video_time_s: number): SampleRow {
  return {
    id: 1,
    recording_id: 1,
    game_time_s,
    video_time_s,
    our_team: null,
    gold_diff: null,
    kill_diff: null,
    cs_diff: null,
    our_gold: null,
    our_level: null,
  };
}

describe("measureGameStart", () => {
  it("is the offset between the game clock and the video clock", () => {
    // Capture ran for 12s before the game clock started.
    expect(measureGameStart([sample(0, 12), sample(30, 42)])).toBe(12);
  });

  it("measures from the lowest game time, not the first element", () => {
    // Samples arrive ordered, but nothing here should depend on that.
    expect(measureGameStart([sample(30, 42), sample(0, 12)])).toBe(12);
  });

  it("returns zero when there is nothing to measure", () => {
    expect(measureGameStart([])).toBe(0);
  });

  it("refuses to skip a lead-in too short to be one", () => {
    // Below MIN_SKIP_S there is no loading screen worth cutting, and cutting
    // it anyway would trim real footage off the front.
    expect(measureGameStart([sample(0, MIN_SKIP_S - 0.5)])).toBe(0);
    expect(measureGameStart([sample(0, MIN_SKIP_S)])).toBe(MIN_SKIP_S);
  });

  it("returns zero for a reconnect", () => {
    // Capture started *after* the game did, so the offset is negative and
    // there is no loading screen in front of it to skip.
    expect(measureGameStart([sample(600, 0)])).toBe(0);
  });
});

describe("measureGameEnd", () => {
  it("is the last sample's video position", () => {
    expect(measureGameEnd([sample(0, 12), sample(30, 42), sample(15, 27)])).toBe(42);
  });

  it("is null, not zero, when there are no samples", () => {
    // Load-bearing: null means "not measured" and makes `viewingWindow` fall
    // back to the end of the file. Zero would clip the whole recording away.
    expect(measureGameEnd([])).toBeNull();
  });
});

describe("viewingWindow", () => {
  it("keeps a lead-in in front of the game", () => {
    const w = viewingWindow(30, 600, 660);
    expect(w.start).toBe(30 - LEAD_IN_S);
  });

  it("never starts before the file does", () => {
    // A game starting inside the lead-in must not produce a negative start.
    expect(viewingWindow(0.5, 600, 660).start).toBe(0);
  });

  it("clips the post-game tail", () => {
    const w = viewingWindow(30, 600, 640);
    expect(w.end).toBe(600);
    expect(w.span).toBe(600 - (30 - LEAD_IN_S));
  });

  it("falls back to the end of the file when there is nothing to measure", () => {
    // No samples. The whole file is the window rather than a guess.
    expect(viewingWindow(0, null, 660).end).toBe(660);
  });

  it("falls back to the end of the file when the tail is already short", () => {
    expect(viewingWindow(30, 660, 660).end).toBe(660);
  });

  it("refuses to clip a gap too large to be a post-game tail", () => {
    // A stretch where Live Client Data answered with something unparseable
    // keeps recording and produces no samples, so real gameplay sits after
    // the last one. Cutting there would hide the game.
    const fileEnd = 600 + MAX_TAIL_CLIP_S + 1;
    expect(viewingWindow(30, 600, fileEnd).end).toBe(fileEnd);
  });

  it("clips a gap exactly at the limit", () => {
    const fileEnd = 600 + MAX_TAIL_CLIP_S;
    expect(viewingWindow(30, 600, fileEnd).end).toBe(600);
  });

  it("never ends before it starts", () => {
    // A measured end earlier than the lead-in would otherwise give a negative
    // span, and every fraction derived from it would be nonsense. The tail
    // has to be short enough to be clipped at all, or the guard above returns
    // the end of the file and the question never arises.
    const w = viewingWindow(300, 290, 300);
    expect(w.end).toBe(w.start);
    expect(w.span).toBe(0);
  });

  it("is empty for a duration that is not a number yet", () => {
    // `video.duration` is NaN until metadata loads. Passing that through
    // would make every derived number NaN, silently.
    expect(viewingWindow(30, 600, Number.NaN)).toEqual({ start: 0, end: 0, span: 0 });
    expect(viewingWindow(30, 600, Number.POSITIVE_INFINITY).span).toBe(0);
    expect(viewingWindow(30, 600, 0).span).toBe(0);
  });
});

describe("displayTime", () => {
  it("reads zero at the window's start", () => {
    const w = viewingWindow(30, 600, 660);
    expect(displayTime(w.start, w)).toBe(0);
  });

  it("never goes negative inside the skipped lead", () => {
    const w = viewingWindow(30, 600, 660);
    expect(displayTime(0, w)).toBe(0);
  });
});

describe("windowFraction", () => {
  it("runs 0 to 1 across the window", () => {
    const w = viewingWindow(1, 101, 101);
    expect(windowFraction(w.start, w)).toBe(0);
    expect(windowFraction(w.end, w)).toBe(1);
    expect(windowFraction(w.start + w.span / 2, w)).toBeCloseTo(0.5);
  });

  it("clamps outside the window", () => {
    // A marker the window excludes is drawn at the edge rather than outside
    // the track.
    const w = viewingWindow(30, 600, 660);
    expect(windowFraction(0, w)).toBe(0);
    expect(windowFraction(10_000, w)).toBe(1);
  });

  it("is zero rather than NaN for an empty window", () => {
    expect(windowFraction(42, { start: 0, end: 0, span: 0 })).toBe(0);
  });
});

describe("clamp", () => {
  it("bounds on both sides and passes the middle through", () => {
    expect(clamp(-1, 0, 10)).toBe(0);
    expect(clamp(11, 0, 10)).toBe(10);
    expect(clamp(5, 0, 10)).toBe(5);
  });
});
