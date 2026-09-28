import { describe, expect, it } from "vitest";
import { gameClockAt, noteTimeAt, videoAt } from "./clock";

const at = (video_time_s: number, game_time_s: number) => ({ video_time_s, game_time_s });

describe("gameClockAt", () => {
  it("applies the offset of the nearest point with both clocks", () => {
    // A 20-second loading screen: game time runs 20 s behind the file.
    expect(gameClockAt(212, [at(200, 180), at(260, 240)])).toBe(192);
  });

  it("follows a drift rather than one fixed offset", () => {
    // After a pause the offset grows; the later point is the nearer one.
    expect(gameClockAt(500, [at(100, 80), at(490, 440)])).toBe(450);
  });

  it("reads markers as well as samples", () => {
    expect(gameClockAt(70, [], [at(60, 40)])).toBe(50);
  });

  it("never goes below zero on the loading screen", () => {
    expect(gameClockAt(5, [at(20, 0)])).toBe(0);
  });

  it("is unknown with nothing to measure from", () => {
    expect(gameClockAt(100, [], [])).toBeNull();
  });
});

describe("videoAt and noteTimeAt", () => {
  // Recording started 8 s before the game clock; a pause at 10:00 of game
  // time moved the offset by another 30 s.
  const markers = [
    { game_time_s: 60, video_time_s: 68 },
    { game_time_s: 900, video_time_s: 938 },
  ];

  it("places a game time through the latest point before it", () => {
    expect(videoAt(120, null, markers)).toBe(128);
    // Still before the pause's point, so still the first offset.
    expect(videoAt(880, null, markers)).toBe(888);
    expect(videoAt(1000, null, markers)).toBe(1038);
    // Before every point: the earliest one governs.
    expect(videoAt(10, null, markers)).toBe(18);
  });

  it("falls back to the stored offset when nothing is clocked", () => {
    // Game clock was -8 s at the first frame.
    expect(videoAt(120, -8000, [])).toBe(128);
  });

  it("reads a note in video time on a recording nothing ever clocked", () => {
    expect(videoAt(42, null, [])).toBe(42);
  });

  it("makes a note that reads back at the moment it was made", () => {
    for (const [offset, sources] of [
      [null, [markers]],
      [-8000, [[]]],
      [null, [[]]],
    ] as const) {
      // Before every point, between the points, and after the pause.
      for (const at of [30, 500, 1200]) {
        const stamped = noteTimeAt(at, offset, ...sources);
        expect(videoAt(stamped, offset, ...sources)).toBeCloseTo(at, 6);
      }
    }
  });

  it("is off by at most the pause inside a gap that holds one", () => {
    // Somewhere between the two markers the game paused for 30 s, and
    // nothing says when, so video 930 is either game 922 (the pause is still
    // to come) or game 892 (it has happened). A note made there can read back
    // up to the pause's length away. Real recordings carry a sample every
    // second, which shrinks that gap to about a second.
    const back = videoAt(noteTimeAt(930, null, markers), null, markers);
    expect(Math.abs(back - 930)).toBeLessThanOrEqual(30);
  });
});
