import { listen } from "@tauri-apps/api/event";

import { initDesktop } from "./desktop";
import App from "./lib/App.svelte";
import { initCaptureNotices } from "./lib/stores/capture.svelte";
import { initDaemonStatus, whenDaemonReachable } from "./lib/stores/daemon.svelte";
import { applyDefaultSort, refreshDiskUsage, refreshLibrary } from "./lib/stores/library.svelte";
import { quitEverything } from "./lib/stores/quit.svelte";
import { syncFromPrefs } from "./lib/stores/settings.svelte";
import { initStatus } from "./lib/stores/status.svelte";
import { checkOnOpen, onStatusEvent, refreshUpdateStatus } from "./lib/stores/update.svelte";
import { loadPrefs } from "./prefs";
import { initRouting, mountApp, onViewChange } from "./router";
import { applyThemePref, initTheme } from "./theme";

/**
 * The composition root - and since WS4.6, very nearly all that is left of the
 * vanilla frontend.
 *
 * It owns nothing. `index.html` is one `<div>` and a boot script; every
 * element the app has is rendered by `App.svelte`, which is why there is not a
 * single `el()` call below. That is deliberate and worth keeping: WS4.4 left
 * one behind after deleting the markup it looked up, `el` throws on a miss by
 * design, and the whole frontend failed to boot with every gate green.
 * `main.boot.test.ts` is what watches for that now.
 */
window.addEventListener("DOMContentLoaded", () => {
  // The theme is already on <html> from the inline boot script; this adopts
  // that value into module state and starts following the OS.
  initTheme();

  // Before anything else binds a listener: these are `document`-level
  // suppressions of browser behaviour, and none of them depend on anything
  // existing yet.
  initDesktop();

  // Before the views: if the recorder is not running, that is the first thing
  // worth saying, and the views will be showing stale or empty data because
  // of it. Both are polls and subscriptions rather than elements now, so
  // neither cares whether the root has mounted.
  initDaemonStatus();
  // What the last recording lost to a capture failure (#10). A subscription,
  // like the one above, so it cares nothing for whether the root has mounted.
  initCaptureNotices();
  initStatus();
  void refreshUpdateStatus();

  // Reads the URL fragment. `App.svelte` registers each view when it mounts,
  // which is after this, so `registerView` adopts whatever is current rather
  // than assuming a fresh node's default. See `router.ts`.
  initRouting();

  mountApp(App, document.querySelector("#app-root") as HTMLElement);

  // **The panel is only read when someone opens it, so that is when it is
  // worth being right.** The background loop runs every six hours and nothing
  // else re-checked, which meant a correct "Up to date." from hours ago read
  // as a broken updater. Rate-limited inside `checkOnOpen`, because Settings
  // is one click from the library and gets revisited.
  onViewChange((view) => {
    if (view === "settings") void checkOnOpen();
  });

  // Pushed by the background check in `lib.rs`, which runs on a six-hourly
  // loop, far too slow to poll for. `.catch` because `listen` rejects outside
  // the Tauri webview, which `bridge.ts` deliberately supports.
  listen("update-status-changed", () => {
    void onStatusEvent();
  }).catch((err) => console.warn("update-status-changed listener unavailable:", err));

  // The close action is `quit`, and `lib.rs` has vetoed the close so this can
  // run. Two processes have to stop, in order, and the person may have to be
  // asked first: `quit.ts` owns all of that.
  listen("quit-requested", () => {
    void quitEverything();
  }).catch((err) => console.warn("quit-requested listener unavailable:", err));

  // The backend pushes this after a finalize, after a retention deletion, and
  // after any dev-portal write. Before it existed, a recording the supervisor
  // had just finished stayed invisible until the user happened to press
  // Refresh.
  listen("library-changed", () => {
    void refreshLibrary();
    void refreshDiskUsage();
  }).catch((err) => console.warn("library-changed listener unavailable:", err));

  // **Not on load: on connect.** Every one of these is an RPC to the daemon,
  // and on a cold start the window paints before the handshake finishes; on a
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
    // paint. Every consumer re-applies rather than waiting on them.
    void loadPrefs().then((prefs) => {
      applyThemePref(prefs.theme);
      syncFromPrefs();
      applyDefaultSort(prefs.defaultSort);
    });
  });
});
