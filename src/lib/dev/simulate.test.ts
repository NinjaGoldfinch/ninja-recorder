import { describe, expect, it } from "vitest";
import { GAME_STATES } from "../../dev/types";
import { DEFAULT_REPLAY_EVENTS, EVENTS } from "./simulate";

describe("state events", () => {
  it("cover a full game, in the order the hint gives", () => {
    // The panel's hint-block promises this exact path, so it is the one
    // thing here that can be wrong without anybody noticing.
    const path = ["Client opened", "Phase: InProgress", "Live Client up", "Phase: EndOfGame"];
    const order = path.map((label) => EVENTS.findIndex((e) => e.label === label));
    expect(order).not.toContain(-1);
    expect([...order]).toEqual([...order].sort((a, b) => a - b));
  });

  it("all carry a kind, which is what the backend matches on", () => {
    for (const e of EVENTS) expect(typeof e.event.kind).toBe("string");
  });

  it("all say what they do, since the note is the dispatch result's text", () => {
    for (const e of EVENTS) expect(e.note.length).toBeGreaterThan(20);
  });

  it("name only phases, not states - a gameflow phase is not a GameState", () => {
    const phases = EVENTS.filter((e) => e.event.kind === "gameflow_phase").map(
      (e) => e.event.phase,
    );
    expect(phases).toEqual(["ChampSelect", "InProgress", "Reconnect", "EndOfGame"]);
    for (const phase of phases) {
      expect(GAME_STATES).not.toContain(phase as (typeof GAME_STATES)[number]);
    }
  });

  it("have unique labels, because the buttons are keyed by them", () => {
    expect(new Set(EVENTS.map((e) => e.label)).size).toBe(EVENTS.length);
  });
});

describe("the default replay timeline", () => {
  it("is in ascending game time, which is what a tick walks", () => {
    const times = DEFAULT_REPLAY_EVENTS.map((e) => e.event_time);
    expect([...times]).toEqual([...times].sort((a, b) => a - b));
  });

  it("repeats a timestamp, so cross-poll de-duplication is exercised", () => {
    const times = DEFAULT_REPLAY_EVENTS.map((e) => e.event_time);
    expect(new Set(times).size).toBeLessThan(times.length);
  });

  it("fits inside the default 1200s game length", () => {
    for (const e of DEFAULT_REPLAY_EVENTS) expect(e.event_time).toBeLessThan(1200);
  });

  it("names an event for every entry", () => {
    for (const e of DEFAULT_REPLAY_EVENTS) expect(e.event_name).toBeTruthy();
  });
});
