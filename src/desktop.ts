/**
 * The browser behaviours a desktop window does not want.
 *
 * A Tauri app inherits every reflex the webview has as a *page*: right-click
 * offers to reload or save the video, F5 throws the whole UI away, Ctrl+P
 * prints the library, icons peel off as drag images, and middle-click opens
 * Chromium's autoscroll puck over a layout with nothing to scroll. None of
 * these are features of this app; they are just the platform showing
 * through.
 *
 * This module owns the *event* half of hiding it. The selection half is CSS
 * — see the "This is a window, not a page" block in `styles.css` — because
 * only CSS can hand selection back per element.
 *
 * Both halves are deliberately narrow: they suppress browser chrome, never
 * app behaviour. The hotkeys in `review.ts` are untouched, and every
 * suppression here has an exemption for the one place it would be a
 * regression (a text field, or a build that can inspect itself).
 */
import { hasDevCommands } from "./bridge";

/**
 * A build you can inspect keeps the native context menu — "Inspect element"
 * is the whole point of it — and keeps reload, which is the fastest way to
 * see a frontend change. True for the vite dev server, and for a `devtools`
 * build once the probe answers.
 *
 * Read at event time rather than captured at init, so the few milliseconds
 * before the probe resolves simply behave like a shipped build instead of
 * having to block startup on IPC.
 */
let inspectable = import.meta.env.DEV;

/**
 * Somewhere the user can type. The exemption is the same for all three
 * suppressions below: in a text field the context menu is the ordinary
 * Cut/Copy/Paste one a desktop app would show anyway, and dragging a
 * selection within it is editing, not a stray gesture.
 */
function isEditable(target: EventTarget | null): boolean {
  const el = target instanceof Element ? target : null;
  return !!el?.closest("input, textarea, [contenteditable]");
}

export function initDesktop() {
  void hasDevCommands().then((yes) => {
    inspectable ||= yes;
  });

  document.addEventListener("contextmenu", (e) => {
    if (inspectable || isEditable(e.target)) return;
    e.preventDefault();
  });

  // `user-select: none` stops the highlight, not the drag: the brand mark,
  // the marker icons and the <video> itself still lift off under the
  // cursor, and there is nowhere in the window to drop them.
  document.addEventListener("dragstart", (e) => {
    if (isEditable(e.target)) return;
    e.preventDefault();
  });

  // Middle-click autoscroll.
  document.addEventListener("mousedown", (e) => {
    if (e.button === 1) e.preventDefault();
  });

  document.addEventListener("keydown", (e) => {
    const mod = e.ctrlKey || e.metaKey;
    const key = e.key.toLowerCase();

    // Reload is the damaging one. It discards the open VOD, the poll timer
    // and any half-typed settings, while the backend hears nothing — a
    // recording carries on, and the UI comes back looking like a fresh
    // launch mid-game.
    if (!inspectable && (e.key === "F5" || (mod && key === "r"))) {
      e.preventDefault();
      return;
    }

    // The print dialog renders the app as a document, which it is not.
    if (mod && key === "p") e.preventDefault();
  });
}
