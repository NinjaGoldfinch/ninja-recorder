/**
 * The updater's state - WS4 task 4.4.
 *
 * **Quiet about interrupting, not about telling.** CI publishes a release for
 * every commit on `main`, so "something newer exists" is true most days, and a
 * toast or a system notification on each one is noise the user learns to
 * dismiss without reading. The announcement is a dot and nothing more.
 *
 * What the panel says is a separate question, and the answer is: as much as it
 * has. The version, and what changed, because deciding whether to restart
 * mid-session is the user's call and they cannot make it from a version number
 * alone. See DEVELOPMENT.md §14.
 *
 * The one thing that interrupts is the backend **refusing** an install, since
 * the user pressed a button and is owed an answer. A failed download reports
 * itself in the row instead: they are already looking at it.
 */

import { call } from "../../bridge";
import type { UpdateStatus } from "../../types";
import { type UpdateRow, updateRow } from "../settings/update";
import { toast } from "./toast.svelte";

let status = $state<UpdateStatus | null>(null);
let installing = $state(false);
let checking = $state(false);
let lastCheckedAt = 0;

/**
 * How recently a check has to have run for opening Settings not to trigger
 * another.
 *
 * Opening Settings checks, because the background loop only runs every six
 * hours and the panel would otherwise show an answer up to that stale, which
 * is the exact situation that made a correct "Up to date." look like a broken
 * updater. But Settings is one click from the library and gets opened
 * repeatedly, so a bare "check on open" is a request per visit.
 */
const RECHECK_AFTER_MS = 60_000;

export const update = {
  /** What the row says, or null before the first answer. */
  get row(): UpdateRow | null {
    return status === null ? null : updateRow(status);
  },
  get installing() {
    return installing;
  },
  get checking() {
    return checking;
  },
  /** Whether the dot on the settings button should show. */
  get offering(): boolean {
    return this.row?.offering ?? false;
  },
};

export async function refreshUpdateStatus(): Promise<void> {
  try {
    const next = await call<UpdateStatus>("get_update_status");
    // **A download in flight owns the row**: a background check landing
    // mid-install must not put the buttons back under the user. A failure is
    // the exception, because it is the install's own outcome arriving.
    if (installing && next.kind !== "failed") return;
    if (next.kind === "failed") installing = false;
    status = next;
  } catch (err) {
    // A build with no updater answers `unsupported` rather than rejecting, so
    // reaching here means the command itself failed. Worth a console line and
    // nothing louder: nobody asked.
    console.warn("could not read the update status:", err);
  }
}

/** Called when the background check pushes `update-status-changed`. */
export async function onStatusEvent(): Promise<void> {
  lastCheckedAt = Date.now();
  await refreshUpdateStatus();
}

export async function checkNow(): Promise<void> {
  checking = true;
  try {
    await call("check_for_update");
    // The command returns as soon as the request is handed over; the answer
    // arrives on the event. Nothing to render here.
  } catch (err) {
    toast(`Could not check for updates: ${String(err)}`, "error");
  } finally {
    checking = false;
  }
}

/**
 * Opening Settings is when the panel is read, so it is when it is worth being
 * right. Rate-limited, because Settings is one click away and gets revisited.
 */
export async function checkOnOpen(): Promise<void> {
  await refreshUpdateStatus();
  if (installing) return;
  if (Date.now() - lastCheckedAt < RECHECK_AFTER_MS) return;
  await checkNow();
}

/** The channel changed, so the panel must stop describing the one they left. */
export async function recheckForChannel(): Promise<void> {
  lastCheckedAt = 0;
  status = { kind: "checking" } as UpdateStatus;
  await checkNow();
}

export async function install(): Promise<void> {
  // No gate check here: the button is only enabled when the last render said
  // it was installable, and `core::install_update` checks again anyway. A game
  // can start between a glance and a click, and only the backend is positioned
  // to know.
  installing = true;
  status = { kind: "checking" } as UpdateStatus;
  try {
    await call("install_update");
    // `install_update` returns the instant the request is handed over, not
    // when the download finishes, so this is the last thing said here. The
    // process is replaced by the installer if it works; if it does not, the
    // backend records the failure and the event brings us back with it.
  } catch (err) {
    // The backend declined outright: a game started between the render and the
    // click, which is the case the second gate check exists for.
    installing = false;
    toast(String(err), "error");
    await refreshUpdateStatus();
  }
}

/** Overrides the row's text while a download is in flight. */
export function installingText(): string {
  return "Downloading the update…";
}
