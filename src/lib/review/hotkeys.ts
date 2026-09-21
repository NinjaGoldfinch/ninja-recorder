/**
 * Which key does what in the review player.
 *
 * Extracted from `review.ts`'s `handleHotkey` by WS4.5. The decision and the
 * acting on it were one `else if` chain that called six different functions;
 * separating them is what makes the guards testable, and the guards are the
 * part that has already been got wrong once.
 *
 * **These are the only marker navigation available in fullscreen.** The rich
 * timeline lives outside `.player-wrap`, so it is not rendered there.
 */

export type HotkeyAction =
  | "togglePlay"
  | "seekForward"
  | "seekBack"
  | "toggleFullscreen"
  | "toggleMute"
  | "nextMarker"
  | "prevMarker"
  | "nextDeath"
  | "prevDeath"
  | "closeMenu";

/** What the page is doing, as far as a keystroke is concerned. */
export interface HotkeyContext {
  /** Whether the review view is the one showing. */
  reviewOpen: boolean;
  /** Focus is in an `<input>` or `<textarea>`. */
  typing: boolean;
  /** Focus is on a control that does something of its own with Space. */
  onFormControl: boolean;
  /** Focus is on a control that does something of its own with the arrows. */
  onArrowControl: boolean;
  /** The speed and track menu is open. */
  menuOpen: boolean;
}

/** How far the arrow keys seek. */
export const SEEK_STEP_S = 5;

/**
 * The action a key should trigger, or null to let it through.
 *
 * Null means "not ours": the caller must not `preventDefault` on it.
 */
export function hotkeyAction(key: string, ctx: HotkeyContext): HotkeyAction | null {
  if (!ctx.reviewOpen || ctx.typing) return null;

  // **Space is the browser's own way to press a focused button or open a
  // focused select**, so stealing it would break every control in the row the
  // moment one had focus.
  if (key === " " && ctx.onFormControl) return null;

  // **Arrows are not.** A `<button>` ignores them entirely, so waving them off
  // for any focused button bought nothing and cost everything: clicking a
  // timeline glyph, which *is* a button, left it focused and killed seeking
  // until you happened to click somewhere else. A `<select>` does use them,
  // and a focused `<input>` is already out by `typing`, which is what keeps
  // the volume slider's own arrow handling.
  if (key.startsWith("Arrow") && ctx.onArrowControl) return null;

  switch (key) {
    case " ":
      return "togglePlay";
    case "ArrowRight":
      return "seekForward";
    case "ArrowLeft":
      return "seekBack";
    case "f":
      return "toggleFullscreen";
    case "m":
      return "toggleMute";
    case "]":
      return "nextMarker";
    case "[":
      return "prevMarker";
    case "d":
      return "prevDeath";
    case "D":
      return "nextDeath";
    case "Escape":
      // **Guarded on the menu being open** so this never shadows the user
      // agent's own Escape-exits-fullscreen.
      return ctx.menuOpen ? "closeMenu" : null;
    default:
      return null;
  }
}
