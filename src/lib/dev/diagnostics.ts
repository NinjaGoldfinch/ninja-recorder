/**
 * What is wrong with a recording, read off what its finalize observed.
 *
 * `recordings.diagnostics_json` (migration 7) is written at finalize because
 * none of it survives the game: Live Client Data is gone the moment it ends.
 *
 * **The point is to lead with what is wrong or missing rather than dumping
 * fields.** A row of eleven numbers is not an answer; "we were never found in
 * allPlayers, which is why this card has no champion" is. That translation was
 * inline in a panel that also built HTML, so it had no test.
 */

import type { RecordingDiagnostics, RecordingRow } from "../../dev/types";

export function parseDiagnostics(row: RecordingRow): RecordingDiagnostics | null {
  if (!row.diagnostics_json) return null;
  try {
    return JSON.parse(row.diagnostics_json) as RecordingDiagnostics;
  } catch {
    return null;
  }
}

/** How long after the last successful poll the recording kept running. */
export function pollingStoppedEarlyBy(row: RecordingRow, d: RecordingDiagnostics): number | null {
  if (row.duration_s === null || d.last_game_time_s === null || d.alignment_offset_s === null) {
    return null;
  }
  return row.duration_s - (d.last_game_time_s + d.alignment_offset_s);
}

/** Past this, polling dying before the recorder did is worth saying. */
export const POLLING_GAP_S = 10;

/**
 * What is worth saying about this recording, worst first.
 *
 * Empty means nothing looked wrong, which is itself worth printing: a panel
 * that says nothing is indistinguishable from one that failed to look.
 */
export function concerns(row: RecordingRow, d: RecordingDiagnostics): string[] {
  const out: string[] = [];

  if (d.polls === 0) {
    out.push(
      "The Live Client Data poller never got a single snapshot, so there are no markers, no samples and no champion.",
    );
  } else if (!d.ever_matched) {
    out.push(
      "We were never found in allPlayers \u2014 which is exactly why this recording has no champion, no KDA and an empty advantage curve.",
    );
  }

  if (d.game_id === null) {
    out.push(
      "The client never said which game this was, so queue, role and patch can never be filled in for it.",
    );
  }

  if (d.alignment_offset_s === null && d.polls > 0) {
    out.push(
      "The game clock was never seen to advance, so markers fall back to a 1:1 alignment and may sit in the wrong place.",
    );
  }

  // The #74 fingerprint: polling died well before the recorder did.
  const gap = pollingStoppedEarlyBy(row, d);
  if (gap !== null && gap > POLLING_GAP_S) {
    out.push(
      `Polling stopped about ${gap.toFixed(0)}s before the recording did \u2014 the endpoint went away while the capture kept running.`,
    );
  }

  if (/fail/i.test(d.backend)) {
    out.push(`The capture backend reported itself as "${d.backend}".`);
  }

  return out;
}

/** What to call a recording that may have no champion. */
export function recordingName(row: RecordingRow): string {
  return row.champion ?? row.path.split(/[\\/]/).pop() ?? `recording ${row.id}`;
}
