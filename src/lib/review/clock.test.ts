import { describe, expect, it } from "vitest";
import { gameClockAt } from "./clock";

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
