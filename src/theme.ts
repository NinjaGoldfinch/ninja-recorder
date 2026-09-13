import { cachedTheme, savePref, type ThemePref } from "./prefs";

// JS owns <html data-theme> and writes only "light" or "dark" — the OS
// preference is resolved here rather than in CSS. That keeps one dark block
// in the stylesheet instead of two, and makes an explicit "Light" on a dark
// OS win by construction instead of by specificity.
const media = window.matchMedia("(prefers-color-scheme: dark)");

let pref: ThemePref = "system";

function resolve(value: ThemePref): "light" | "dark" {
  if (value === "system") return media.matches ? "dark" : "light";
  return value;
}

function apply() {
  document.documentElement.dataset.theme = resolve(pref);
}

export function themePref(): ThemePref {
  return pref;
}

export function initTheme() {
  pref = cachedTheme();
  apply();
  // Taking ownership of the attribute costs us the one thing the media
  // query did for free: following the OS live. This puts it back.
  media.addEventListener("change", () => {
    if (pref === "system") apply();
  });
}

// Adopt the value loaded from the DB without writing it back.
export function applyThemePref(value: ThemePref) {
  pref = value;
  apply();
}

export function setThemePref(value: ThemePref) {
  applyThemePref(value);
  savePref("theme", value);
}
