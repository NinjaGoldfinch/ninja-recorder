import { listen } from "@tauri-apps/api/event";

import { initDaemonStatus, whenDaemonReachable } from "./daemon";
import { initDesktop } from "./desktop";
import { initDevPortal } from "./devportal";
import { el } from "./dom";
import { applyDefaultSort, initLibrary, refreshDiskUsage, refreshLibrary } from "./library";
import { loadPrefs } from "./prefs";
import { initReview } from "./review";
import { initRouting, registerView } from "./router";
import { initSettings, syncSettingsFromPrefs } from "./settings";
import { initStatus } from "./status";
import { applyThemePref, initTheme } from "./theme";
import { initToast } from "./toast";
import { initUpdate } from "./update";

window.addEventListener("DOMContentLoaded", () => {
  // The theme is already on <html> from the inline boot script; this adopts
  // that value into module state and starts following the OS.
  initTheme();

  // Before anything else binds a listener: these are `document`-level
  // suppressions of browser behaviour, and none of them depend on the views
  // existing.
  initDesktop();

  registerView("library", el("#library-view"));
  registerView("review", el("#review-view"));
  registerView("settings", el("#settings-view"));

  initToast();
  // Before the views: if the recorder is not running, that is the first thing
  // worth saying, and the views below will be showing stale or empty data
  // because of it.
  initDaemonStatus();
  initLibrary();
  initReview();
  initSettings();
  initStatus();
  initDevPortal();
  // After `initToast`: a *refused* install — a game started between the
  // render and the click — is the one thing this reports loudly, and it
  // reports it through the toast.
  initUpdate();

  // After the views are registered, so a `#settings` start or a tray
  // "Settings" click has something to switch to.
  initRouting();

  // The backend pushes this after a finalize, after a retention deletion,
  // and after any dev-portal write. Before it existed, a recording the
  // supervisor had just finished stayed invisible until the user happened
  // to press Refresh. `.catch` because `listen` rejects outright outside
  // the Tauri webview, and bridge.ts deliberately supports running there.
  listen("library-changed", () => {
    void refreshLibrary();
    void refreshDiskUsage();
  }).catch((err) => console.warn("library-changed listener unavailable:", err));

  // **Not on load: on connect.** Every one of these is an RPC to the daemon,
  // and on a cold start the window paints before the handshake finishes — on a
  // first launch after an install it is starting the daemon itself, which
  // takes seconds. Fetching here got `not connected to the recorder` back and
  // reported it as an error, so a healthy install greeted its owner with a red
  // box that cleared itself moments later.
  //
  // It runs on every *re*connect too, which is the half that was missing
  // entirely: WS3.8 made the strip clear itself when the daemon came back, but
  // nothing re-read the library, so a window that lost its recorder went on
  // showing whatever it held when the connection died.
  whenDaemonReachable(() => {
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
});
