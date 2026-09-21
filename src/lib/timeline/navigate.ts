/**
 * Seeking from one marker to the next.
 *
 * `[` and `]` and `d` and `D` are the only marker navigation available in
 * fullscreen, because the rich timeline lives outside `.player-wrap` and is
 * not rendered there. So this is the whole of that affordance, and worth a
 * test of its own.
 */

import type { MarkerRow } from "../../types";

/**
 * A small deadband around the current position.
 *
 * Without it, "next" pressed while sitting exactly on a marker re-selects the
 * same one and nothing appears to happen. A quarter of a second is under the
 * point a seek is noticeable and well over the precision `currentTime` reports
 * after one.
 */
export const DEADBAND_S = 0.25;

/**
 * The marker to seek to, or null when there are none to seek to.
 *
 * **Wraps at both ends**: `]` past the last marker goes back to the first, and
 * `[` before the first goes to the last. A key that stops working at the edge
 * reads as broken, and there is nothing else it could usefully do.
 *
 * `markers` must already be ordered by video time, which is how `get_markers`
 * returns them.
 */
export function nextMarker(
  markers: readonly MarkerRow[],
  currentTimeS: number,
  direction: 1 | -1,
  predicate: (m: MarkerRow) => boolean = () => true,
): MarkerRow | null {
  const candidates = markers.filter(predicate);
  if (candidates.length === 0) return null;

  if (direction === 1) {
    return candidates.find((m) => m.video_time_s > currentTimeS + DEADBAND_S) ?? candidates[0];
  }
  return (
    [...candidates].reverse().find((m) => m.video_time_s < currentTimeS - DEADBAND_S) ??
    candidates[candidates.length - 1]
  );
}
