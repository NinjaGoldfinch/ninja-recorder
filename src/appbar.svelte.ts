/**
 * The two app-bar controls that outlived their views - WS4 task 4.4.
 *
 * The settings button and the update dot sit in `index.html`'s header, which
 * is not a view and is not migrated: WS4.6 deletes it along with the rest of
 * the markup. Until then something has to own them, and the alternative was
 * leaving `update.ts` alive purely to toggle one element's `hidden`.
 *
 * `$effect.root` is what lets a plain module read a rune. The root is never
 * torn down, which is correct here: these elements live as long as the window
 * does, and the app-bar markup is the thing being kept alive rather than a
 * component with a lifecycle.
 */

import { listen } from "@tauri-apps/api/event";
import { el } from "./dom";
import {
  checkOnOpen,
  onStatusEvent,
  refreshUpdateStatus,
  update,
} from "./lib/stores/update.svelte";
import { onViewChange, showView } from "./router";

export function initAppBar() {
  el<HTMLButtonElement>("#open-settings-btn").addEventListener("click", () => {
    showView("settings");
  });

  const badge = el("#update-badge");
  $effect.root(() => {
    $effect(() => {
      badge.hidden = !update.offering;
    });
  });

  // Pushed by the background check in `lib.rs`, which runs on a six-hourly
  // loop, far too slow to poll for. `.catch` because `listen` rejects outside
  // the Tauri webview, which `bridge.ts` deliberately supports.
  listen("update-status-changed", () => {
    void onStatusEvent();
  }).catch((err) => console.warn("update-status-changed listener unavailable:", err));

  // **The panel is only read when someone opens it, so that is when it is
  // worth being right.** The background loop runs every six hours and nothing
  // else re-checked, which meant a correct "Up to date." from hours ago read
  // as a broken updater. Rate-limited inside `checkOnOpen`, because Settings
  // is one click from the library and gets revisited.
  onViewChange((view) => {
    if (view === "settings") void checkOnOpen();
  });

  void refreshUpdateStatus();
}
