/**
 * The line of facts under the review heading: when, how long, which queue,
 * how it went and where it left us on the ladder.
 *
 * Each fact is included only when known, so a rescan import with nothing
 * but a file date reads "27 Sept, 12:30" rather than a row of placeholders.
 */

import {
  formatClock,
  formatDateTime,
  formatKda,
  lpLabel,
  queueOrModeLabel,
  rankLabel,
} from "../../format";
import type { RecordingRow } from "../../types";

export function reviewFacts(row: RecordingRow): string[] {
  const facts: string[] = [formatDateTime(row.started_at)];
  if (row.duration_s !== null && row.duration_s > 0) facts.push(formatClock(row.duration_s));
  const queue = queueOrModeLabel(row);
  if (queue) facts.push(queue);
  const kda = formatKda(row.kda_k, row.kda_d, row.kda_a);
  if (kda) facts.push(kda);
  if (row.cs !== null) facts.push(`${row.cs} CS`);
  const rank = rankLabel(row.tier, row.division);
  if (rank) {
    // LP only beside a rank, as on the library row: a number with no scale is
    // not a standing.
    const lp = lpLabel(row.lp_after);
    facts.push(lp ? `${rank} ${lp}` : rank);
  }
  return facts;
}
