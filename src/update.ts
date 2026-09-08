import { listen } from "@tauri-apps/api/event";

import { call } from "./bridge";
import { el } from "./dom";
import { getPrefs, savePref, type UpdateChannelPref } from "./prefs";
import { onViewChange } from "./router";
import { toast } from "./toast";
import type { UpdateStatus } from "./types";

/**
 * The update row in Settings → About, and the dot on the settings button.
 *
 * Quiet about *interrupting*, not about *telling*. CI publishes a release for
 * every commit on `main`, so "something newer exists" is true most days, and a
 * toast or a system notification on each one is noise the user learns to
 * dismiss without reading. So the announcement is a dot and nothing more.
 *
 * What the panel itself says is a separate question, and the answer is: as
 * much as it has. The version, and what changed, because deciding whether to
 * restart mid-session is the user's call and they cannot make it from a
 * version number alone. See DEVELOPMENT.md §14.
 *
 * The one thing that interrupts is the backend *refusing* an install — a game
 * started between the render and the click — because the user pressed a button
 * and is owed an answer. A failed download reports itself in the row instead:
 * they are already looking at it.
 */

interface Els {
  channel: HTMLSelectElement;
  text: HTMLElement;
  notes: HTMLElement;
  install: HTMLButtonElement;
  badge: HTMLElement;
  check: HTMLButtonElement;
}

let els: Els;
let installing = false;
let lastCheckedAt = 0;

/**
 * How recently a check has to have run for opening Settings not to trigger
 * another.
 *
 * Opening Settings checks, because the background loop only runs every six
 * hours and the panel would otherwise show an answer up to that stale — the
 * exact situation that made a correct "Up to date." look like a broken
 * updater. But Settings is one click from the library and gets opened
 * repeatedly, so a bare "check on open" is a request per visit.
 */
const RECHECK_AFTER_MS = 60_000;

export function initUpdate() {
  els = {
    channel: el("#update-channel"),
    text: el("#about-update"),
    notes: el("#about-update-notes"),
    install: el("#update-install"),
    badge: el("#update-badge"),
    check: el("#update-check"),
  };

  els.install.addEventListener("click", () => void install());
  els.check.addEventListener("click", () => void checkNow());
  els.channel.addEventListener("change", () => {
    const value = els.channel.value as UpdateChannelPref;
    savePref("updateChannel", value);
    // Check straight away rather than leaving the panel describing the
    // channel they just left. `savePref` is fire-and-forget, but Rust reads
    // the pref from SQLite when the check runs, so the write has to land
    // first — hence awaiting the round trip here and nowhere else.
    els.text.textContent = "Checking\u2026";
    lastCheckedAt = 0;
    void checkNow();
  });

  // Pushed by the background check in `lib.rs`, which runs on a six-hourly
  // loop — far too slow to poll for. `.catch` because `listen` rejects
  // outside the Tauri webview, which `bridge.ts` deliberately supports.
  listen("update-status-changed", () => {
    lastCheckedAt = Date.now();
    void refreshUpdateStatus();
  }).catch((err) =>
    console.warn("update-status-changed listener unavailable:", err),
  );

  // The panel is only read when someone opens it, so that is when it is
  // worth being right. The background loop runs every six hours and nothing
  // else re-checked, which meant a correct "Up to date." from hours ago read
  // as a broken updater.
  onViewChange((view) => {
    if (view !== "settings") return;
    void refreshUpdateStatus();
    if (installing) return;
    if (Date.now() - lastCheckedAt < RECHECK_AFTER_MS) return;
    void checkNow();
  });

  syncChannelControl();
  void refreshUpdateStatus();
}

/** Called on load and whenever prefs resolve from the database. */
export function syncChannelControl() {
  if (!els) return;
  els.channel.value = getPrefs().updateChannel;
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

/**
 * Renders the changelog as **text nodes**, never markup.
 *
 * `latest.json` is fetched over HTTPS but is not covered by the update
 * signature — only the installer it points at is — so everything in here is
 * remote text this app did not write. Building nodes rather than assigning
 * `innerHTML` means there is no escaping to get wrong.
 *
 * The format CI writes is a `## What's changed` heading, `- ` bullets per
 * commit, and a trailing full-changelog line. The heading is dropped (the row
 * is already labelled), bullets become a list, and anything else is a
 * paragraph — an unrecognized line is shown rather than swallowed.
 */
function renderNotes(notes: string | null) {
  els.notes.replaceChildren();
  const lines = (notes ?? "")
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line.length > 0 && !line.startsWith("#"));
  if (lines.length === 0) {
    els.notes.hidden = true;
    return;
  }

  let list: HTMLUListElement | null = null;
  for (const line of lines) {
    if (line.startsWith("- ")) {
      list ??= els.notes.appendChild(document.createElement("ul"));
      const item = document.createElement("li");
      item.textContent = line.slice(2);
      list.appendChild(item);
      continue;
    }
    list = null;
    const para = document.createElement("p");
    para.textContent = line;
    els.notes.appendChild(para);
  }
  els.notes.hidden = false;
}

function render(status: UpdateStatus) {
  const offering = status.kind === "available";
  els.badge.hidden = !offering;
  els.install.hidden = !offering;
  if (!offering) els.notes.hidden = true;
  // A download in flight owns the row: a background check landing mid-install
  // must not re-enable the buttons under the user.
  if (installing && status.kind !== "failed") return;
  els.check.disabled = false;

  switch (status.kind) {
    case "checking":
      els.text.textContent = "Checking\u2026";
      break;
    case "unsupported":
      // Not "you are up to date": this build will never find out. A
      // devtools bundle and anything built off Windows both land here.
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
      renderNotes(status.offer.notes);
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
  // The changelog stays up. It is what the user was reading to decide, and
  // removing it the instant they act is the one moment it is least welcome.
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
