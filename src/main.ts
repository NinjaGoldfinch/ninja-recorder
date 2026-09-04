import { el } from "./dom";
import { applyDefaultSort, initLibrary, refreshDiskUsage, refreshLibrary } from "./library";
import { loadPrefs } from "./prefs";
import { registerView } from "./router";
import { initReview } from "./review";
import { initSettings, syncSettingsFromPrefs } from "./settings";
import { initStatus } from "./status";
import { applyThemePref, initTheme } from "./theme";
import { initToast } from "./toast";

window.addEventListener("DOMContentLoaded", () => {
  // The theme is already on <html> from the inline boot script; this adopts
  // that value into module state and starts following the OS.
  initTheme();

  registerView("library", el("#library-view"));
  registerView("review", el("#review-view"));
  registerView("settings", el("#settings-view"));

  initToast();
  initLibrary();
  initReview();
  initSettings();
  initStatus();

  void refreshLibrary();
  void refreshDiskUsage();

  // Preferences come from SQLite, so they land a beat after the first
  // paint. Both consumers re-apply rather than waiting on them.
  void loadPrefs().then((prefs) => {
    applyThemePref(prefs.theme);
    syncSettingsFromPrefs();
    applyDefaultSort();
  });
});
