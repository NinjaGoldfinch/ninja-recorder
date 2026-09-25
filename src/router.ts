import { listen } from "@tauri-apps/api/event";
import { type Component, mount, unmount } from "svelte";

// Which top-level section is showing. Previously each view toggled its own
// and its sibling's `hidden` directly, from two different files, so the
// library's visibility was written in places that knew nothing about each
// other.

// `game` is the WS9 review form for one game and `objectives` the list it
// reviews against. `game`, like `review`, is never a start fragment: both need
// something chosen first, and a cold window has nothing chosen.
export type View = "library" | "review" | "settings" | "game" | "objectives";

const views = new Map<View, HTMLElement>();
const listeners: ((view: View) => void)[] = [];
let current: View = "library";

export function registerView(name: View, node: HTMLElement) {
  views.set(name, node);
  // **Adopt the current state, do not assume it.** Views used to be registered
  // in one run before anything could switch, so a fresh node's `hidden` was
  // always right by default. Since WS4.3 a migrated view registers itself when
  // its component mounts, which is after `initRouting` has read the URL
  // fragment: a window opened at `#settings` would otherwise show the settings
  // section *and* the library, because the library's node missed the
  // `showView` that hid everything else.
  node.hidden = name !== current;
}

function isView(value: string): value is View {
  return (
    value === "library" ||
    value === "review" ||
    value === "settings" ||
    value === "game" ||
    value === "objectives"
  );
}

// The tray's "Settings" item has to work whether the window already exists or
// is being created for the click. A live window gets a `navigate` event; a cold
// one is opened at `index.html#settings`, because the frontend cannot be
// listening for an event it only subscribes to once it has loaded.
export function initRouting() {
  const start = window.location.hash.replace(/^#/, "");
  if (isView(start) && start !== "review" && start !== "game") showView(start);

  void listen<string>("navigate", (event) => {
    if (isView(event.payload)) showView(event.payload);
  }).catch((err) => console.warn("navigate listener unavailable:", err));
}

export function currentView(): View {
  return current;
}

export function onViewChange(cb: (view: View) => void) {
  listeners.push(cb);
}

// Hiding stays on the `hidden` attribute rather than a class: the review
// player's document-level hotkeys are gated on which view is showing, and
// a mechanism that disagrees with what the router thinks would leave `[`
// and `]` seeking a video nobody can see.
export function showView(name: View) {
  if (name === current) return;
  for (const [key, node] of views) node.hidden = key !== name;
  current = name;
  for (const cb of listeners) cb(name);
}

// ---------------------------------------------------------------------------
// The Svelte seam — WS4 task 4.1.
//
// WS4 is a strangler, so for its length the window runs two frontends at once:
// the vanilla sections `index.html` still holds, and one Svelte root mounted
// beside them. The router owns the join because it already owns the only
// question both halves have to agree on — which view is showing — and because
// a second module toggling `hidden` is the exact failure `showView` was written
// to end.
//
// The root goes up once and stays up. It is deliberately *not* mounted and
// unmounted as views change: `mount` and `unmount` destroy component state, so
// driving them from `showView` would throw away a migrated view's scroll
// position and in-flight requests every time the user glanced at Settings.
// Visibility stays what it has always been, the `hidden` attribute on the host.

let root: Record<string, unknown> | null = null;

/**
 * Bring the Svelte root up inside `host`, once.
 *
 * Idempotent on purpose. `main.ts` calls it from `DOMContentLoaded`, and a
 * second call mounting a second copy of the tree into the same node is a bug
 * that shows up as duplicated UI rather than as an error, so it is refused
 * here instead.
 */
export function mountApp(component: Component, host: HTMLElement) {
  if (root) return;
  root = mount(component, { target: host });
}

/**
 * Take it down again, and report whether there was anything to take down.
 *
 * `outro: false` skips leaving transitions: every caller is tearing the
 * window down or resetting between tests, and neither wants to wait on an
 * animation. Nothing in the app calls this during normal use — it exists so
 * that mounting is reversible, which is what makes the root testable and what
 * the WS4.1 exit criterion is actually about.
 */
export async function unmountApp(): Promise<boolean> {
  if (!root) return false;
  const mounted = root;
  // Cleared before awaiting, so a `mountApp` racing the teardown cannot see a
  // root that is already on its way out and refuse to replace it.
  root = null;
  await unmount(mounted, { outro: false });
  return true;
}

/** Whether the Svelte root is currently up. */
export function appMounted(): boolean {
  return root !== null;
}
