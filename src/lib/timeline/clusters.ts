/**
 * Marker clustering: several markers close together become one glyph.
 *
 * Extracted from `review.ts`'s `renderGlyphs` by WS4.2. The clustering itself
 * was inline in a function that also measured the DOM, built HTML strings and
 * assigned `innerHTML`, which is why it had no test despite being the part
 * most likely to be wrong.
 *
 * Markers around a teamfight land within a few pixels of each other at any
 * sensible zoom. Drawing them individually gives an unreadable smear and a
 * click target smaller than a finger, so they collapse into one badge-counted
 * glyph.
 */

import type { MarkerRow } from "../../types";

/**
 * Markers closer than this on screen collapse into one glyph.
 *
 * Pixels rather than the percentage the old two-lane strip used: a percentage
 * threshold means something completely different on a 600px-wide window than
 * on a 1600px one, and the finger that has to hit the glyph is the same size
 * on both.
 */
export const CLUSTER_PX = 28;

/**
 * When several markers collapse into one glyph, this is which icon wins.
 *
 * Ordered by how much the event changes what you are looking for in a VOD:
 * your own deaths and kills first, then objectives by value, with assists
 * last because they are the most numerous and the least individually
 * interesting.
 */
export const MARKER_PRIORITY = [
  "multikill",
  "death",
  "kill",
  "baron",
  "dragon",
  "herald",
  "voidgrubs",
  "inhibitor",
  "ace",
  "first_blood",
  "turret",
  "assist",
];

/**
 * Groups `markers` into clusters by how far apart they are on screen.
 *
 * `xOf` maps a marker to its horizontal position in pixels, which is what
 * keeps this function free of both the window maths and the DOM. It is
 * called once per marker.
 *
 * **Markers must already be ordered by video time**, which is how
 * `get_markers` returns them. The comparison is against the pixel position
 * where the current cluster *started*, not the previous marker's, so a dense
 * run cannot chain into one cluster spanning the whole track: each marker is
 * measured against the cluster's own origin.
 */
export function clusterMarkers(
  markers: readonly MarkerRow[],
  xOf: (marker: MarkerRow) => number,
  clusterPx: number = CLUSTER_PX,
): MarkerRow[][] {
  const clusters: MarkerRow[][] = [];
  let clusterStartX = Number.NEGATIVE_INFINITY;

  for (const marker of markers) {
    const px = xOf(marker);
    if (clusters.length > 0 && px - clusterStartX <= clusterPx) {
      clusters[clusters.length - 1].push(marker);
    } else {
      clusters.push([marker]);
      clusterStartX = px;
    }
  }

  return clusters;
}

/** Where `kind` sits in the priority order; unknown kinds sort last. */
export function markerRank(kind: string, priority: readonly string[] = MARKER_PRIORITY): number {
  const i = priority.indexOf(kind);
  return i === -1 ? priority.length : i;
}

/**
 * The marker whose icon represents a whole cluster.
 *
 * Copies before sorting: the cluster is the array the caller is about to
 * render and reordering it would move the glyph, since its position comes
 * from `cluster[0]`.
 */
export function leadMarker(
  cluster: readonly MarkerRow[],
  priority: readonly string[] = MARKER_PRIORITY,
): MarkerRow {
  return [...cluster].sort(
    (a, b) => markerRank(a.kind, priority) - markerRank(b.kind, priority),
  )[0];
}

/** Where a cluster's glyph is drawn: the mean of its markers' video times. */
export function clusterCentre(cluster: readonly MarkerRow[]): number {
  return cluster.reduce((sum, m) => sum + m.video_time_s, 0) / cluster.length;
}
