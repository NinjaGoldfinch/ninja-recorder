/**
 * The advantage graph, and the ruler's tick spacing.
 *
 * WS4.2 took the downsampling and the tick maths; WS4.5 took the rest, which
 * was a function with five early returns that each wrote different text into
 * different elements. `graphView` returns which of those five a recording is
 * in, so the component renders one of five things instead of reaching into
 * three elements to undo what the last call did.
 */

import type { SampleRow } from "../../types";
import type { ViewingWindow } from "./window";

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

// --- The advantage curve ---------------------------------------------------

/** What the metric picker offers. `none` hides the curve without hiding it. */
export type MetricKey = "none" | "gold_diff" | "kill_diff" | "cs_diff";

export function formatSigned(value: number): string {
  return `${value > 0 ? "+" : ""}${Math.round(value)}`;
}

export function formatSignedGold(value: number): string {
  const sign = value > 0 ? "+" : value < 0 ? "-" : "";
  const abs = Math.abs(value);
  return abs >= 1000 ? `${sign}${(abs / 1000).toFixed(1)}k` : `${sign}${Math.round(abs)}`;
}

export const METRIC_META: Record<
  Exclude<MetricKey, "none">,
  { label: string; empty: string; format: (v: number) => string }
> = {
  gold_diff: {
    label: "Gold diff",
    // Its own message, because its absence has its own cause. Gold comes from
    // the post-game match timeline, so a custom or a practice game never has
    // one, and a real game does not have one until the deferred patch lands.
    // Falling back to the generic "not enough data" would read as a bug in all
    // three cases.
    empty: "No gold data for this recording",
    format: formatSignedGold,
  },
  kill_diff: { label: "Kill diff", empty: "Not enough data to plot", format: formatSigned },
  cs_diff: { label: "CS diff", empty: "Not enough data to plot", format: formatSigned },
};

export function metricValue(sample: SampleRow, metric: MetricKey): number | null {
  switch (metric) {
    case "gold_diff":
      return sample.gold_diff;
    case "kill_diff":
      return sample.kill_diff;
    case "cs_diff":
      return sample.cs_diff;
    default:
      return null;
  }
}

/**
 * What the timeline should show for a recording.
 *
 * Five states, and they are not interchangeable. `noSamples` is a recording
 * made before the poller existed; `noSide` is one where we were never matched
 * in `allPlayers`, so every diff's sign is unknowable; `noMetric` is the user
 * choosing to hide the curve; `tooSparse` is a metric that exists but has
 * fewer than two points. Only the last draws anything.
 *
 * The distinction that matters most is `noSide`. Showing the curve anyway
 * would risk telling someone they were ahead in a game they lost.
 */
export type GraphView =
  | { kind: "noSamples"; summary: string; picker: "hidden" }
  | { kind: "noSide"; summary: string; picker: "disabled" }
  | { kind: "noMetric"; summary: string; picker: "enabled" }
  | { kind: "tooSparse"; summary: string; picker: "enabled" }
  | { kind: "curve"; summary: string; picker: "enabled"; line: string; area: string };

/** The SVG viewBox this draws into, which the component must match. */
export const GRAPH_WIDTH = 1000;
export const GRAPH_HEIGHT = 100;

export function graphView(
  samples: readonly SampleRow[],
  metric: MetricKey,
  window: ViewingWindow,
  windowFraction: (t: number, w: ViewingWindow) => number,
): GraphView {
  if (samples.length === 0) {
    return { kind: "noSamples", summary: "No metric data for this recording", picker: "hidden" };
  }

  const side = samples.find((s) => s.our_team)?.our_team ?? null;
  if (!side) {
    return {
      kind: "noSide",
      summary: "Team side unknown \u2014 diff unavailable",
      picker: "disabled",
    };
  }

  if (metric === "none") return { kind: "noMetric", summary: "", picker: "enabled" };

  const meta = METRIC_META[metric];
  const points = samples
    .map((s) => ({ t: s.video_time_s, v: metricValue(s, metric) }))
    .filter((p): p is { t: number; v: number } => p.v !== null);

  if (points.length < 2) {
    return { kind: "tooSparse", summary: meta.empty, picker: "enabled" };
  }

  const reduced = downsample(points, 500);
  const bound = Math.max(...reduced.map((p) => Math.abs(p.v)));
  const x = (t: number) => windowFraction(t, window) * GRAPH_WIDTH;
  // **Symmetric about the baseline, not min-to-max.** A min-to-max scale
  // floats the zero crossing, so a game spent entirely behind renders as a
  // line through the middle and reads as "even". 5 units of headroom each side.
  const mid = GRAPH_HEIGHT / 2;
  const y = (v: number) => (bound === 0 ? mid : mid - (v / bound) * 45);

  const line = reduced
    .map((p, i) => `${i === 0 ? "M" : "L"}${x(p.t).toFixed(2)} ${y(p.v).toFixed(2)}`)
    .join(" ");
  // Filled between the curve and the zero line rather than down to the bottom
  // edge, so the ahead/behind split is what communicates the swing.
  const area = `M${x(reduced[0].t).toFixed(2)} ${mid} ${line.slice(1)} L${x(
    reduced[reduced.length - 1].t,
  ).toFixed(2)} ${mid} Z`;

  const last = reduced[reduced.length - 1].v;
  const peak = reduced.reduce((a, p) => (Math.abs(p.v) > Math.abs(a) ? p.v : a), 0);

  return {
    kind: "curve",
    summary: `${meta.label} \u00b7 ${meta.format(last)} at end \u00b7 peak ${meta.format(peak)}`,
    picker: "enabled",
    line,
    area,
  };
}
