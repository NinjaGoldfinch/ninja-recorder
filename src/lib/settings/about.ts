/**
 * The three live lines in Settings → About.
 *
 * `status.ts` polls for these and used to write them straight into elements
 * inside the settings markup, which is how a module that owns the app bar's
 * pills ended up owning three rows of a view it has nothing to do with. WS4.4
 * leaves the polling where it is and moves the wording here, so the strings
 * can be tested and the component can read them from a store.
 */

import type { LcuStatus, SupervisorStatus } from "../../types";

export function lcuLine(status: LcuStatus): string {
  if (status.error) return `Error: ${status.error}`;
  if (!status.connected) return "Not running (no lockfile found).";
  // The LCU hands back an empty string, not null, when it has no Riot ID for
  // us yet, so this falls back on falsiness rather than nullishness.
  const who = status.summoner || "signed in";
  return `Connected as ${who} — phase ${status.phase ?? "?"}.`;
}

/**
 * What was finalized last, for the About block.
 *
 * **A null `recording_id` means the file exists but its row never got
 * written**, which is worth saying out loud rather than rendering as a blank.
 */
export function finalizedLine(status: SupervisorStatus): string {
  const finalized = status.last_finalized;
  if (!finalized) return "None yet.";
  const idNote =
    finalized.recording_id === null ? "DB WRITE FAILED" : `db id ${finalized.recording_id}`;
  return `${finalized.path} (${finalized.markers.length} markers, ${idNote})`;
}
