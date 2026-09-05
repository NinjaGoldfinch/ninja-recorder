import { call } from "./bridge";

export type ThemePref = "system" | "light" | "dark";
export type SortKey = "newest" | "oldest" | "longest" | "champion";

export interface Prefs {
  theme: ThemePref;
  defaultSort: SortKey;
}

export const DEFAULT_PREFS: Prefs = {
  theme: "system",
  defaultSort: "newest",
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

function isTheme(value: unknown): value is ThemePref {
  return value === "system" || value === "light" || value === "dark";
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
  };

  try {
    const stored = await call<Record<string, string>>("get_ui_prefs");
    if (isTheme(stored.theme)) prefs.theme = stored.theme;
    if (isSort(stored.defaultSort)) prefs.defaultSort = stored.defaultSort;
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
