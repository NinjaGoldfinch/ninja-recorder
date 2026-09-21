/**
 * Reading a recording's scoreboard: who we played, and who we played against.
 *
 * Moved out of `library.ts` by WS4.3 so that `review.ts` and the Svelte row
 * can share it without either importing the other. It was already the one
 * function the row, the heading and the accessible name all went through, and
 * that is the property worth keeping: they cannot disagree about who the
 * opponent was.
 */

import { parseScoreboard } from "../../icons";
import type { RecordingRow, ScoreboardPlayer } from "../../types";

/** Us, as the scoreboard recorded us. */
export function selfPlayer(row: RecordingRow): ScoreboardPlayer | null {
  return parseScoreboard(row.scoreboard_json)?.players.find((p) => p.is_us) ?? null;
}

/**
 * The one opponent who played our position, or null.
 *
 * **A lookup, never a guess.** Every player carries a `position`, so the enemy
 * in our lane is a filter over the board rather than an index into a list
 * whose order nothing promises. Null where the position is missing, which is
 * every recording made before the field existed and any mode with no positions
 * to assign, because an arbitrary enemy is worse than none.
 */
export function laneOpponent(row: RecordingRow): ScoreboardPlayer | null {
  const players = parseScoreboard(row.scoreboard_json)?.players ?? [];
  const us = players.find((p) => p.is_us) ?? null;
  if (!us?.position) return null;
  return players.find((p) => p.team !== us.team && p.position === us.position) ?? null;
}

/** "8.3 /min", or null when either half is missing. */
export function csPerMinute(row: RecordingRow): string | null {
  if (row.cs === null || row.duration_s === null || row.duration_s <= 0) return null;
  return `${(row.cs / (row.duration_s / 60)).toFixed(1)} /min`;
}

/** What the row's `data-outcome` attribute carries. */
export function outcomeAttr(win: boolean | null): "win" | "loss" | "unknown" {
  if (win === null) return "unknown";
  return win ? "win" : "loss";
}
