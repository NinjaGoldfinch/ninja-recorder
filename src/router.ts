import { listen } from "@tauri-apps/api/event";

// Which top-level section is showing. Previously each view toggled its own
// and its sibling's `hidden` directly, from two different files, so the
// library's visibility was written in places that knew nothing about each
// other.

export type View = "library" | "review" | "settings";

const views = new Map<View, HTMLElement>();
const listeners: ((view: View) => void)[] = [];
let current: View = "library";

export function registerView(name: View, node: HTMLElement) {
  views.set(name, node);
}

function isView(value: string): value is View {
  return value === "library" || value === "review" || value === "settings";
}

// The tray's "Settings" item has to work whether the window already exists or
// is being created for the click. A live window gets a `navigate` event; a cold
// one is opened at `index.html#settings`, because the frontend cannot be
// listening for an event it only subscribes to once it has loaded.
export function initRouting() {
  const start = window.location.hash.replace(/^#/, "");
  if (isView(start) && start !== "review") showView(start);

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
