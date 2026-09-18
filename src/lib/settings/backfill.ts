/**
 * What the backfill says it did.
 *
 * Moved out of `settings.ts` by WS4.4. It was already pure and already had the
 * reasoning written down; what it did not have was a test, because it lived
 * next to the element it was assigned to.
 */

import type { BackfillReport } from "../../types";

/**
 * Says what happened to every row, not just the ones that worked.
 *
 * **"Nothing to fill in" and "none of them matched" look identical if only the
 * successes are reported**, and they call for opposite reactions: the second
 * means the recordings are older than the client's history and no amount of
 * re-running will help. Ambiguous rows get their own sentence because refusing
 * to guess is a decision, not a failure.
 */
export function backfillSummary(report: BackfillReport): string {
  if (report.scanned === 0) {
    return "Nothing to fill in — every recording already has its match data.";
  }

  const parts = [
    `Checked ${report.scanned} recording${report.scanned === 1 ? "" : "s"} against ` +
      `${report.games_considered} game${report.games_considered === 1 ? "" : "s"} of match history.`,
  ];
  parts.push(report.patched > 0 ? `Filled in ${report.patched}.` : "Nothing could be filled in.");
  // Counted separately from `patched` because they fail independently: the
  // gold timeline is a different endpoint, and a game can still yield its
  // metadata after the timeline has aged out of the client's history.
  if (report.gold_filled > 0) {
    parts.push(`Recovered the gold curve for ${report.gold_filled}.`);
  }
  if (report.ambiguous > 0) {
    parts.push(
      `${report.ambiguous} overlapped more than one game and were left alone rather than guessed at.`,
    );
  }
  if (report.unmatched > 0) {
    parts.push(
      `${report.unmatched} matched no game — they are older than your client's history, or were customs.`,
    );
  }
  return parts.join(" ");
}
