import { describe, expect, it } from "vitest";
import { type HotkeyContext, hotkeyAction } from "./hotkeys";

const ctx = (over: Partial<HotkeyContext> = {}): HotkeyContext => ({
  reviewOpen: true,
  typing: false,
  onFormControl: false,
  onArrowControl: false,
  menuOpen: false,
  ...over,
});

describe("hotkeyAction", () => {
  it("maps each key to its action", () => {
    expect(hotkeyAction(" ", ctx())).toBe("togglePlay");
    expect(hotkeyAction("ArrowRight", ctx())).toBe("seekForward");
    expect(hotkeyAction("ArrowLeft", ctx())).toBe("seekBack");
    expect(hotkeyAction("f", ctx())).toBe("toggleFullscreen");
    expect(hotkeyAction("m", ctx())).toBe("toggleMute");
    expect(hotkeyAction("]", ctx())).toBe("nextMarker");
    expect(hotkeyAction("[", ctx())).toBe("prevMarker");
    expect(hotkeyAction("d", ctx())).toBe("prevDeath");
    expect(hotkeyAction("D", ctx())).toBe("nextDeath");
  });

  it("does nothing outside the review view", () => {
    expect(hotkeyAction(" ", ctx({ reviewOpen: false }))).toBeNull();
  });

  it("does nothing while typing", () => {
    expect(hotkeyAction(" ", ctx({ typing: true }))).toBeNull();
    expect(hotkeyAction("d", ctx({ typing: true }))).toBeNull();
  });

  describe("Space and focused controls", () => {
    it("leaves Space to a focused button or select", () => {
      // Space is the browser's own way to press a focused button. Stealing it
      // would break every control in the row the moment one had focus.
      expect(hotkeyAction(" ", ctx({ onFormControl: true }))).toBeNull();
    });

    it("still takes the arrows there", () => {
      // A `<button>` ignores arrows entirely.
      expect(hotkeyAction("ArrowRight", ctx({ onFormControl: true }))).toBe("seekForward");
    });
  });

  describe("the arrows and a focused select", () => {
    it("leaves the arrows to a select, which uses them", () => {
      expect(hotkeyAction("ArrowRight", ctx({ onArrowControl: true }))).toBeNull();
      expect(hotkeyAction("ArrowLeft", ctx({ onArrowControl: true }))).toBeNull();
    });

    it("still takes Space there only if it is not also a form control", () => {
      // A select is both in practice; this pins that the two guards are
      // independent rather than one condition doing double duty.
      expect(hotkeyAction("]", ctx({ onArrowControl: true }))).toBe("nextMarker");
    });
  });

  describe("Escape", () => {
    it("closes the menu when it is open", () => {
      expect(hotkeyAction("Escape", ctx({ menuOpen: true }))).toBe("closeMenu");
    });

    it("is left alone when the menu is shut", () => {
      // Otherwise it shadows the user agent's own Escape-exits-fullscreen.
      expect(hotkeyAction("Escape", ctx({ menuOpen: false }))).toBeNull();
    });
  });

  it("returns null for a key it does not own", () => {
    // Null means "not ours", and the caller must not preventDefault on it.
    expect(hotkeyAction("k", ctx())).toBeNull();
    expect(hotkeyAction("Tab", ctx())).toBeNull();
  });
});
