import { describe, expect, it } from "vitest";
import type { MarkerRow } from "../../types";
import { currentItem, currentRow, eventRows, listItems, matchesFilter } from "./events";

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

describe("listItems and currentItem", () => {
  const m = (id: number, video_time_s: number, kind = "kill") =>
    ({
      id,
      recording_id: 1,
      game_time_s: video_time_s,
      video_time_s,
      kind,
      payload_json: "{}",
    }) as MarkerRow;
  const placed = (id: number, videoTimeS: number) => ({
    note: {
      id,
      game_id: 1,
      ts_ms: videoTimeS * 1000,
      kind: "mistake" as const,
      body: "b",
      created_at: 0,
    },
    videoTimeS,
  });

  it("merges notes into the events in playback order, the event first at a tie", () => {
    const rows = eventRows([m(1, 10), m(2, 100)], new Set(), "all");
    const items = listItems(rows, [placed(7, 50), placed(8, 100)], "all");
    expect(
      items.map((i) => (i.type === "event" ? `e${i.row.marker.id}` : `n${i.placed.note.id}`)),
    ).toEqual(["e1", "n7", "e2", "n8"]);
  });

  it("shows only notes under Notes, and no notes under an event filter", () => {
    const markers = [m(1, 10), m(2, 20, "death")];
    const notesOnly = listItems(eventRows(markers, new Set(), "notes"), [placed(7, 5)], "notes");
    expect(notesOnly.map((i) => i.type)).toEqual(["note"]);
    const deaths = listItems(eventRows(markers, new Set(), "deaths"), [placed(7, 5)], "deaths");
    expect(deaths.map((i) => i.type)).toEqual(["event"]);
  });

  it("lights the last item the playhead passed", () => {
    const items = listItems(eventRows([m(1, 10)], new Set(), "all"), [placed(7, 30)], "all");
    expect(currentItem(items, 5)).toBe(-1);
    expect(currentItem(items, 20)).toBe(0);
    expect(currentItem(items, 31)).toBe(1);
  });
});
