import { describe, expect, it } from "vitest";
import type { MarkerRow } from "../../types";
import { currentRow, eventRows, matchesFilter } from "./events";

let nextId = 1;
const m = (kind: string, t: number, payload: Record<string, unknown> = {}): MarkerRow => ({
  id: nextId++,
  recording_id: 1,
  game_time_s: t,
  video_time_s: t + 20,
  kind,
  payload_json: JSON.stringify(payload),
});

describe("matchesFilter", () => {
  it("sorts kinds into fights, deaths and objectives", () => {
    expect(matchesFilter("assist", "kills")).toBe(true);
    expect(matchesFilter("death", "kills")).toBe(false);
    expect(matchesFilter("death", "deaths")).toBe(true);
    expect(matchesFilter("voidgrubs", "objectives")).toBe(true);
  });

  it("shows a kind it has never heard of under All only", () => {
    expect(matchesFilter("atakhan", "all")).toBe(true);
    expect(matchesFilter("atakhan", "objectives")).toBe(false);
  });
});

describe("eventRows", () => {
  it("collapses a burst of the same event into one row", () => {
    const rows = eventRows(
      [m("voidgrubs", 649), m("voidgrubs", 656), m("voidgrubs", 660)],
      new Set(),
      "all",
    );
    expect(rows).toHaveLength(1);
    expect(rows[0].count).toBe(3);
    expect(rows[0].marker.game_time_s).toBe(649);
  });

  it("keeps two different kills apart however close they are", () => {
    const rows = eventRows(
      [m("kill", 100, { victim: "Vayne" }), m("kill", 105, { victim: "Talon" })],
      new Set(),
      "all",
    );
    expect(rows.map((r) => r.count)).toEqual([1, 1]);
  });

  it("does not join the same event minutes apart", () => {
    const rows = eventRows([m("voidgrubs", 300), m("voidgrubs", 600)], new Set(), "all");
    expect(rows).toHaveLength(2);
  });

  it("filters before grouping, so a death between grubs does not split them", () => {
    const rows = eventRows(
      [m("voidgrubs", 649), m("death", 650, { killer: "Zed" }), m("voidgrubs", 656)],
      new Set(),
      "objectives",
    );
    expect(rows).toHaveLength(1);
    expect(rows[0].count).toBe(2);
  });

  it("never merges a reachable event with one past the end of the file", () => {
    const a = m("turret", 100);
    const b = m("turret", 110);
    const rows = eventRows([a, b], new Set([b.id]), "all");
    expect(rows.map((r) => r.beyond)).toEqual([false, true]);
  });
});

describe("currentRow", () => {
  const rows = eventRows(
    [m("kill", 100, { victim: "A" }), m("kill", 200, { victim: "B" })],
    new Set(),
    "all",
  );

  it("is -1 before the first event", () => {
    expect(currentRow(rows, 50)).toBe(-1);
  });

  it("is the latest event the playhead has passed", () => {
    expect(currentRow(rows, 150)).toBe(0);
    expect(currentRow(rows, 400)).toBe(1);
  });

  it("lights a row just seeked to, a hair before its time", () => {
    expect(currentRow(rows, 219.8)).toBe(1);
  });
});
