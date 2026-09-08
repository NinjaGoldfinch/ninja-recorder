import { listen } from "@tauri-apps/api/event";

import { call } from "./bridge";
import { el } from "./dom";
import { toast } from "./toast";
import type { UpdateStatus } from "./types";

/**
 * The update row in Settings → About, and the dot on the settings button.
 *
 * Deliberately quiet. CI publishes a release for every commit that lands on
 * `main`, so "something newer exists" is true most days — a toast or a system
 * notification on each one would be noise the user learns to dismiss without
 * reading. The dot is the whole announcement; everything else waits until
 * somebody opens Settings. See DEVELOPMENT.md §14.
 *
 * The one thing that interrupts is the backend *refusing* an install — a game
 * started between the render and the click — because the user pressed a button
 * and is owed an answer. A failed download reports itself in the row instead:
 * they are already looking at it.
 */

interface Els {
  text: HTMLElement;
  install: HTMLButtonElement;
  badge: HTMLElement;
  check: HTMLButtonElement;
}

let els: Els;
let installing = false;

export function initUpdate() {
  els = {
    text: el("#about-update"),
    install: el("#update-install"),
    badge: el("#update-badge"),
    check: el("#update-check"),
  };

  els.install.addEventListener("click", () => void install());
  els.check.addEventListener("click", () => void checkNow());

  // Pushed by the background check in `lib.rs`, which runs on a six-hourly
  // loop — far too slow to poll for. `.catch` because `listen` rejects
  // outside the Tauri webview, which `bridge.ts` deliberately supports.
  listen("update-status-changed", () => {
    void refreshUpdateStatus();
  }).catch((err) =>
    console.warn("update-status-changed listener unavailable:", err),
  );

  void refreshUpdateStatus();
}

export async function refreshUpdateStatus() {
  let status: UpdateStatus;
  try {
    status = await call<UpdateStatus>("get_update_status");
  } catch (err) {
    // A build with no updater answers `unsupported` rather than rejecting,
    // so reaching here means the command itself failed. Worth a line in the
    // console and nothing louder — nobody asked.
    console.warn("could not read the update status:", err);
    return;
  }
  render(status);
}

function render(status: UpdateStatus) {
  const offering = status.kind === "available";
  els.badge.hidden = !offering;
  els.install.hidden = !offering;
  // A download in flight owns the row: a background check landing mid-install
  // must not re-enable the buttons under the user.
  if (installing && status.kind !== "failed") return;
  els.check.disabled = false;

  switch (status.kind) {
    case "unsupported":
      // Not "you are up to date": this build will never find out. A macOS
      // build and a devtools bundle both land here.
      els.text.textContent = "Updates are not available in this build.";
      break;
    case "upToDate":
      els.text.textContent = "Up to date.";
      break;
    case "failed":
      // Printed verbatim: Rust sends a whole sentence, which is what lets a
      // failed check and a failed *install* share this one state without the
      // UI having to guess which it is looking at.
      els.text.textContent = status.error;
      // Whatever the install attempt was, it is over.
      installing = false;
      break;
    case "available": {
      els.install.disabled = !status.installable;
      // `textContent`, not `innerHTML`. There is no markup to build here, and
      // the version string comes out of `latest.json` — remote text. Escaping
      // it would work; not building HTML at all removes the question.
      const why = status.installable
        ? ""
        : ` Cannot install while ${status.blockedReason ?? "the app is busy"}.`;
      els.text.textContent = `Version ${status.offer.version} is available.${why}`;
      break;
    }
  }
}

async function checkNow() {
  els.check.disabled = true;
  try {
    await call("check_for_update");
    // The command returns as soon as the request is handed over; the answer
    // arrives on the event. Nothing to render here.
  } catch (err) {
    toast(`Could not check for updates: ${String(err)}`, "error");
  } finally {
    els.check.disabled = false;
  }
}

async function install() {
  // No gate check here: the button is only enabled when the last render said
  // it was installable, and `core::install_update` checks again anyway — a
  // game can start between a glance and a click, and only the backend is
  // positioned to know.
  installing = true;
  els.install.disabled = true;
  els.check.disabled = true;
  els.text.textContent = "Downloading the update…";
  try {
    await call("install_update");
    // `install_update` returns the instant the request is handed over, not
    // when the download finishes — so this is the last thing said here. The
    // process is replaced by the installer if it works; if it does not, the
    // backend records the failure and the event brings us back through
    // `render` with it.
  } catch (err) {
    // The backend declined outright — a game started between the render and
    // the click, which is the case the second gate check exists for.
    installing = false;
    els.check.disabled = false;
    toast(String(err), "error");
    void refreshUpdateStatus();
  }
}
