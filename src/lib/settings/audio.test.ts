import { describe, expect, it } from "vitest";
import type { AudioInputDevice, AudioPreset } from "../../types";
import { knownPreset, micOptions, presetFor, TRACK_LABELS, trackPreview, usesMic } from "./audio";

describe("usesMic", () => {
  it("knows which presets record one", () => {
    expect(usesMic("game")).toBe(false);
    expect(usesMic("game_mic")).toBe(true);
    expect(usesMic("game_mic_discord")).toBe(true);
    expect(usesMic("desktop")).toBe(false);
  });
});

describe("trackPreview", () => {
  it("numbers the tracks from zero", () => {
    // Track 0 is always the combined mix.
    expect(trackPreview("game_mic")).toBe("Track 0: Everything · Track 1: Game · Track 2: Mic");
  });

  it("leaves Game as a single track", () => {
    // Nothing to isolate, so no combined mix in front of it.
    expect(trackPreview("game")).toBe("Track 0: Game");
  });

  it("covers every preset the toggle can be in", () => {
    for (const key of Object.keys(TRACK_LABELS) as (keyof typeof TRACK_LABELS)[]) {
      expect(trackPreview(key)).toMatch(/^Track 0: /);
    }
  });
});

describe("knownPreset", () => {
  it("passes a known key through", () => {
    expect(knownPreset({ preset: "desktop" } as AudioPreset)).toBe("desktop");
  });

  it("falls back to game for anything it does not have a button for", () => {
    // `custom` has no button yet, and an unknown value can reach us from a
    // newer build's settings row. Neither should leave the toggle showing
    // nothing at all.
    expect(knownPreset({ preset: "custom" } as unknown as AudioPreset)).toBe("game");
    expect(knownPreset({ preset: "from_the_future" } as unknown as AudioPreset)).toBe("game");
  });
});

describe("presetFor", () => {
  it("attaches the microphone only to presets that use one", () => {
    expect(presetFor("game_mic", "mic-1")).toEqual({ preset: "game_mic", mic_device_id: "mic-1" });
    expect(presetFor("game", "mic-1")).toEqual({ preset: "game" });
  });

  it("omits an empty device, which means the Windows default", () => {
    expect(presetFor("game_mic", "")).toEqual({ preset: "game_mic" });
  });
});

describe("micOptions", () => {
  const device = (over: Partial<AudioInputDevice>): AudioInputDevice =>
    ({ id: "a", name: "A", is_default: false, ...over }) as AudioInputDevice;

  it("offers the Windows default first, as an empty id", () => {
    expect(micOptions([])[0]).toEqual({ id: "", label: "Windows default" });
  });

  it("marks which device is currently the default", () => {
    const options = micOptions([device({ id: "x", name: "Mic X", is_default: true })]);
    expect(options[1].label).toBe("Mic X (current default)");
  });

  it("carries a driver's name through as text", () => {
    // A device name comes from the driver, so it is untrusted in the same way
    // a filename is. It reaches a template, not a string of markup.
    const options = micOptions([device({ id: "y", name: "<b>Mic</b>" })]);
    expect(options[1].label).toBe("<b>Mic</b>");
  });
});
