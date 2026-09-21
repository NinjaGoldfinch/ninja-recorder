import { describe, expect, it } from "vitest";
import type { MarkerRow } from "../../types";
import { isElder, markerLabel, markerStyle, multikillLabel } from "./markers";

const marker = (kind: string, payload: unknown = {}): MarkerRow =>
  ({
    id: 1,
    recording_id: 1,
    game_time_s: 0,
    video_time_s: 0,
    kind,
    payload_json: typeof payload === "string" ? payload : JSON.stringify(payload),
  }) as MarkerRow;

describe("markerLabel", () => {
  describe("the three that name somebody", () => {
    it("names who you killed, who killed you, and whose kill you helped with", () => {
      expect(markerLabel(marker("kill", { victim: "Zed" }))).toBe("Killed Zed");
      expect(markerLabel(marker("death", { killer: "Darius" }))).toBe("Killed by Darius");
      expect(markerLabel(marker("assist", { killer: "Lux", victim: "Zed" }))).toBe(
        "Lux killed Zed",
      );
    });

    it("shows a question mark rather than undefined for a missing name", () => {
      expect(markerLabel(marker("kill", {}))).toBe("Killed ?");
      expect(markerLabel(marker("kill", { victim: 42 }))).toBe("Killed ?");
    });
  });

  describe("the objectives, which name nobody", () => {
    it("says just the objective", () => {
      // `classify_event` only writes these when you took part, so the killer
      // is you or an ally, and printing it told you your own champion's name.
      expect(markerLabel(marker("baron", { killer: "Ahri" }))).toBe("Baron");
      expect(markerLabel(marker("herald", {}))).toBe("Herald");
      expect(markerLabel(marker("voidgrubs", {}))).toBe("Voidgrubs");
      expect(markerLabel(marker("turret", {}))).toBe("Turret");
      expect(markerLabel(marker("inhibitor", {}))).toBe("Inhibitor");
    });

    it("marks a steal, and only when it was said out loud", () => {
      // `undefined` is not "it wasn't stolen", it is "nobody asked": the flag
      // is absent on every marker recorded before steals were captured.
      expect(markerLabel(marker("baron", { stolen: true }))).toBe("Baron (stolen)");
      expect(markerLabel(marker("baron", { stolen: false }))).toBe("Baron");
      expect(markerLabel(marker("baron", {}))).toBe("Baron");
      expect(markerLabel(marker("baron", { stolen: "yes" }))).toBe("Baron");
    });

    it("keeps Elder apart from the elemental drakes", () => {
      // The elementals are interchangeable to someone scrubbing a VOD. Elder
      // usually decides the game and is a thing you go looking for by name.
      expect(markerLabel(marker("dragon", { dragon_type: "Elder" }))).toBe("Elder Dragon");
      expect(markerLabel(marker("dragon", { dragon_type: "Fire" }))).toBe("Dragon");
      expect(markerLabel(marker("dragon", {}))).toBe("Dragon");
    });

    it("combines Elder with a steal", () => {
      expect(markerLabel(marker("dragon", { dragon_type: "elder", stolen: true }))).toBe(
        "Elder Dragon (stolen)",
      );
    });
  });

  describe("the ones that were constants dressed as data", () => {
    it("says just Ace and First Blood", () => {
      // `Ace` is only recorded when you landed the closing kill, `FirstBlood`
      // only when you got it, so a "— your team" suffix said nothing.
      expect(markerLabel(marker("ace", { team: "ORDER" }))).toBe("Ace");
      expect(markerLabel(marker("first_blood", {}))).toBe("First Blood");
    });
  });

  it("names a multikill by its streak", () => {
    expect(markerLabel(marker("multikill", { kill_streak: 3 }))).toBe("Triple Kill");
  });

  it("falls back to the kind for something it has never heard of", () => {
    // `kind` is a TEXT column and a newer build can write one this build does
    // not know. A blank label would be worse than the raw word.
    expect(markerLabel(marker("supermassive_objective", {}))).toBe("supermassive_objective");
  });

  it("survives a payload that is not JSON", () => {
    expect(markerLabel(marker("kill", "not json at all"))).toBe("Killed ?");
    expect(markerLabel(marker("baron", "{"))).toBe("Baron");
  });
});

describe("multikillLabel", () => {
  it("uses the names everyone uses", () => {
    expect(multikillLabel(2)).toBe("Double Kill");
    expect(multikillLabel(5)).toBe("Penta Kill");
  });

  it("counts up beyond a penta rather than inventing a word", () => {
    // Either a pentakill already, or a game mode where counting up is the
    // wrong answer. Neither has a name worth guessing.
    expect(multikillLabel(7)).toBe("7× Multikill");
  });

  it("falls back when the streak is not a number", () => {
    expect(multikillLabel(undefined)).toBe("Multikill");
    expect(multikillLabel("three")).toBe("Multikill");
  });
});

describe("isElder", () => {
  it("is case- and whitespace-insensitive", () => {
    expect(isElder({ dragon_type: "  ELDER " })).toBe(true);
    expect(isElder({ dragon_type: "elder" })).toBe(true);
  });

  it("is false for anything else", () => {
    expect(isElder({ dragon_type: "Mountain" })).toBe(false);
    expect(isElder({})).toBe(false);
    expect(isElder({ dragon_type: 1 })).toBe(false);
  });
});

describe("markerStyle", () => {
  it("gives every known kind an icon and a colour", () => {
    expect(markerStyle(marker("kill")).icon).toBe("⚔️");
    expect(markerStyle(marker("death")).color).toBe("#e53935");
  });

  it("still draws a marker it has never heard of", () => {
    // A marker that vanished would be worse than a grey dot.
    const style = markerStyle(marker("from_the_future"));
    expect(style.icon).toBe("●");
    expect(style.label).toBe("from_the_future");
  });
});
