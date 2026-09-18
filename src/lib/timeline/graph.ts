/**
 * The advantage graph's shaping, and the ruler's tick spacing.
 *
 * Extracted from `review.ts` by WS4.2. Both were inline in functions that
 * otherwise measured elements and wrote `innerHTML`.
 */

/**
 * Buckets `points` down to at most `target` entries, keeping the largest
 * magnitude in each bucket.
 *
 * Max-*abs* rather than max: on a signed series the interesting value in a
 * bucket is the biggest swing either way, and plain max would quietly drop
 * every trough, turning a game you were losing into a flat line.
 *
 * A `target` of zero or less returns nothing, and anything already short
 * enough is returned as-is rather than copied.
 */
export function downsample<T extends { v: number }>(points: T[], target: number): T[] {
  if (target <= 0) return [];
  if (points.length <= target) return points;

  const size = points.length / target;
  const out: T[] = [];
  for (let i = 0; i < target; i++) {
    const slice = points.slice(Math.floor(i * size), Math.floor((i + 1) * size));
    if (slice.length === 0) continue;
    out.push(slice.reduce((a, b) => (Math.abs(b.v) > Math.abs(a.v) ? b : a)));
  }
  return out;
}

/**
 * Candidate spacings for labelled ruler ticks, coarsest wins.
 *
 * Every entry divides cleanly by 4, so the minor ticks between them land on
 * whole seconds.
 */
export const RULER_STEPS = [15, 30, 60, 120, 300, 600, 900];

/** At most this many labels on the ruler, whatever the span. */
export const MAX_RULER_LABELS = 16;

/**
 * The gap between labelled ticks for a window of `span` seconds.
 *
 * The first step that keeps the label count under the cap, falling back to
 * the coarsest available for a span longer than any of them can cover. A
 * four-hour recording gets crowded ticks rather than none.
 */
export function rulerStep(
  span: number,
  steps: readonly number[] = RULER_STEPS,
  maxLabels: number = MAX_RULER_LABELS,
): number {
  return steps.find((step) => span / step <= maxLabels) ?? steps[steps.length - 1];
}
