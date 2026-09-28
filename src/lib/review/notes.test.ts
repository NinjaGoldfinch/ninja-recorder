import { describe, expect, it } from "vitest";
import type { MarkerRow } from "../../types";
import type { Note } from "../contract/types";
import { NOTE_KINDS, noteStyle, placeNotes } from "./notes";

function note(id: number, ts_ms: number, created_at = id): Note {
  return { id, game_id: 1, ts_ms, kind: "note", body: `n${id}`, created_at };
}

const marker = (game_time_s: number, video_time_s: number) =>
  ({
    id: 0,
    recording_id: 1,
    game_time_s,
    video_time_s,
    kind: "kill",
    payload_json: "{}",
  }) as MarkerRow;

describe("placeNotes", () => {
  it("places game-time notes in the recording through the markers", () => {
    const placed = placeNotes([note(1, 396_000)], null, [marker(60, 68)], []);
    expect(placed[0]?.videoTimeS).toBe(404);
  });

  it("uses the stored offset when nothing is clocked", () => {
    expect(placeNotes([note(1, 120_000)], -8000, [], [])[0]?.videoTimeS).toBe(128);
  });

  it("orders by position, then by when each note was made", () => {
    const placed = placeNotes(
      [note(1, 90_000, 5), note(2, 30_000), note(3, 90_000, 1)],
      null,
      [],
      [],
    );
    expect(placed.map((p) => p.note.id)).toEqual([2, 3, 1]);
  });
});

describe("noteStyle", () => {
  it("styles every kind, with the neutral default first in the picker", () => {
    expect(NOTE_KINDS[0]).toBe("note");
    for (const kind of NOTE_KINDS) expect(noteStyle(kind).label).not.toBe("");
  });
});
