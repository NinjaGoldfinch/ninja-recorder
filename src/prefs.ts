import { call } from "./bridge";

export type ThemePref = "system" | "light" | "dark";
export type SortKey = "newest" | "oldest" | "longest" | "champion";
// What the window's close button does. Mirrors Rust's `core::CloseAction`,
// which is the side that actually acts on it — this type only has to describe
// the values. "close-window" is the default because a *hidden* window keeps
// the webview fully resident and reclaims nothing.
export type CloseActionPref = "close-window" | "hide" | "quit";
// Which stream of releases this install follows. Mirrors Rust's
// `update::Channel`, which is the side that turns it into an endpoint —
// the two channels are separated by *URL*, never by version comparison,
// because semver says `1.1.0-alpha.1 > 1.0.0` (DEVELOPMENT.md §15).
export type UpdateChannelPref = "stable" | "alpha";
// Notification switches. Stored as "on"/"off" strings rather than booleans
// because `settings_kv` is a text table and Rust parses the same two words.
export type TogglePref = "on" | "off";
export type NotifyPrefKey =
  | "notifications"
  | "notifyRecordingStarted"
  | "notifyRecordingFinished"
  | "notifyRecordingFailed";

export interface Prefs {
  theme: ThemePref;
  updateChannel: UpdateChannelPref;
  defaultSort: SortKey;
  closeAction: CloseActionPref;
  notifications: TogglePref;
  notifyRecordingStarted: TogglePref;
  notifyRecordingFinished: TogglePref;
  notifyRecordingFailed: TogglePref;
  // Not a user-facing toggle: "" means the notice is armed, anything else
  // means it has been shown. Reset blanks it.
  "notice.closeToTray.seen": string;
}

export const DEFAULT_PREFS: Prefs = {
  theme: "system",
  // Stable unless someone deliberately opts in. Rust defaults the same way
  // for anything it cannot parse, so a corrupt value cannot silently put an
  // install on prereleases.
  updateChannel: "stable",
  defaultSort: "newest",
  closeAction: "close-window",
  // These four mirror Rust's `NotificationPrefs::default`; both sides have to
  // agree, because either can be the one that reads a missing key.
  notifications: "on",
  notifyRecordingStarted: "off",
  notifyRecordingFinished: "on",
  notifyRecordingFailed: "on",
  "notice.closeToTray.seen": "",
};

// localStorage is a cache, not the store. Its one job is to be readable
// *synchronously* by the boot script in index.html, before first paint —
// preferences come from SQLite over async IPC, which resolves a frame or
// two too late to pick the theme without a visible flash. SQLite stays the
// source of truth and wins any disagreement.
const CACHE_PREFIX = "nr.";

let prefs: Prefs = { ...DEFAULT_PREFS };

export function getPrefs(): Prefs {
  return prefs;
}

function isToggle(value: unknown): value is TogglePref {
  return value === "on" || value === "off";
}

function isCloseAction(value: unknown): value is CloseActionPref {
  return value === "close-window" || value === "hide" || value === "quit";
}

function isTheme(value: unknown): value is ThemePref {
  return value === "system" || value === "light" || value === "dark";
}

function isUpdateChannel(value: unknown): value is UpdateChannelPref {
  return value === "stable" || value === "alpha";
}

function isSort(value: unknown): value is SortKey {
  return (
    value === "newest" ||
    value === "oldest" ||
    value === "longest" ||
    value === "champion"
  );
}

function readCache<T>(key: string, guard: (v: unknown) => v is T, fallback: T): T {
  try {
    const raw = localStorage.getItem(CACHE_PREFIX + key);
    return guard(raw) ? raw : fallback;
  } catch {
    // Private windows and blocked site data both throw on access.
    return fallback;
  }
}

export function cachedTheme(): ThemePref {
  return readCache("theme", isTheme, DEFAULT_PREFS.theme);
}

// Every value is validated on the way in: a stale or hand-edited row must
// fall back to the default, never take the app down on boot.
export async function loadPrefs(): Promise<Prefs> {
  prefs = {
    theme: cachedTheme(),
    defaultSort: readCache("defaultSort", isSort, DEFAULT_PREFS.defaultSort),
    // Not read from the cache: nothing needs it before first paint, and Rust
    // reads the value straight out of SQLite anyway.
    closeAction: DEFAULT_PREFS.closeAction,
    updateChannel: DEFAULT_PREFS.updateChannel,
    notifications: DEFAULT_PREFS.notifications,
    notifyRecordingStarted: DEFAULT_PREFS.notifyRecordingStarted,
    notifyRecordingFinished: DEFAULT_PREFS.notifyRecordingFinished,
    notifyRecordingFailed: DEFAULT_PREFS.notifyRecordingFailed,
    "notice.closeToTray.seen": DEFAULT_PREFS["notice.closeToTray.seen"],
  };

  try {
    const stored = await call<Record<string, string>>("get_ui_prefs");
    if (isTheme(stored.theme)) prefs.theme = stored.theme;
    if (isSort(stored.defaultSort)) prefs.defaultSort = stored.defaultSort;
    if (isCloseAction(stored.closeAction)) prefs.closeAction = stored.closeAction;
    if (isUpdateChannel(stored.updateChannel)) {
      prefs.updateChannel = stored.updateChannel;
    }
    for (const key of [
      "notifications",
      "notifyRecordingStarted",
      "notifyRecordingFinished",
      "notifyRecordingFailed",
    ] as const) {
      if (isToggle(stored[key])) prefs[key] = stored[key];
    }
  } catch (err) {
    console.error("Failed to load preferences", err);
  }

  for (const [key, value] of Object.entries(prefs)) writeCache(key, value);
  return prefs;
}

function writeCache(key: string, value: string) {
  try {
    localStorage.setItem(CACHE_PREFIX + key, value);
  } catch {
    // Cache-only; the DB write below is what actually persists.
  }
}

export function savePref<K extends keyof Prefs>(key: K, value: Prefs[K]) {
  prefs[key] = value;
  writeCache(key, value);
  // Fire and forget. The caller has already applied the change visually,
  // and awaiting a DB round trip before painting would make a theme toggle
  // feel laggy for no benefit.
  call("set_ui_pref", { key, value }).catch((err) =>
    console.error(`Failed to persist ${key}`, err),
  );
}
