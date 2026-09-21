import { describe, expect, it } from "vitest";
import type { MarkerRow } from "../../types";
import { DEADBAND_S, nextMarker } from "./navigate";

const at = (video_time_s: number, kind = "kill"): MarkerRow =>
  ({
    id: video_time_s,
    recording_id: 1,
    game_time_s: video_time_s,
    video_time_s,
    kind,
    payload_json: "{}",
  }) as MarkerRow;

/**
 * `[`, `]`, `d` and `D` are the only marker navigation available in
 * fullscreen, because the rich timeline is outside `.player-wrap` and is not
 * rendered there. So this is the whole of that affordance.
 */

const markers = [at(10), at(20), at(30)];

describe("nextMarker", () => {
  it("finds the next one forward", () => {
    expect(nextMarker(markers, 15, 1)?.video_time_s).toBe(20);
  });

  it("finds the previous one back", () => {
    expect(nextMarker(markers, 25, -1)?.video_time_s).toBe(20);
  });

  it("advances rather than re-selecting the one you are sitting on", () => {
    // The deadband. Without it, "next" from exactly on a marker picks the same
    // one and nothing appears to happen.
    expect(nextMarker(markers, 20, 1)?.video_time_s).toBe(30);
    expect(nextMarker(markers, 20, -1)?.video_time_s).toBe(10);
  });

  it("treats anything inside the deadband as being on the marker", () => {
    expect(nextMarker(markers, 20 + DEADBAND_S / 2, 1)?.video_time_s).toBe(30);
    expect(nextMarker(markers, 20 - DEADBAND_S / 2, -1)?.video_time_s).toBe(10);
  });

  it("wraps at both ends", () => {
    // A key that stops working at the edge reads as broken, and there is
    // nothing else it could usefully do.
    expect(nextMarker(markers, 999, 1)?.video_time_s).toBe(10);
    expect(nextMarker(markers, 0, -1)?.video_time_s).toBe(30);
  });

  it("is null when there are no markers", () => {
    expect(nextMarker([], 0, 1)).toBeNull();
  });

  it("filters before it navigates", () => {
    // `d` and `D` jump between deaths, which is the one thing people scrub a
    // VOD for most.
    const mixed = [at(10, "kill"), at(20, "death"), at(30, "kill"), at(40, "death")];
    const isDeath = (m: MarkerRow) => m.kind === "death";
    expect(nextMarker(mixed, 0, 1, isDeath)?.video_time_s).toBe(20);
    expect(nextMarker(mixed, 25, 1, isDeath)?.video_time_s).toBe(40);
  });

  it("is null when the filter matches nothing", () => {
    // A game with no deaths: `d` should do nothing rather than seek to 0.
    expect(nextMarker(markers, 0, 1, (m) => m.kind === "death")).toBeNull();
  });

  it("wraps within the filtered set, not the whole list", () => {
    const mixed = [at(10, "kill"), at(20, "death"), at(30, "kill")];
    expect(nextMarker(mixed, 999, 1, (m) => m.kind === "death")?.video_time_s).toBe(20);
  });
});
