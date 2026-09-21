<!--
  The rich timeline: the advantage curve, the marker glyphs, the ruler and the
  playhead.

  **Outside `.player-wrap` on purpose**, which means it is not rendered in
  fullscreen. There, marker navigation is the `[` / `]` / `d` / `D` hotkeys,
  which are bound at the document level and keep working.
-->

<script lang="ts">
import { formatTime } from "../../../format";
import type { MarkerRow } from "../../../types";
import { CLUSTER_PX, clusterCentre, clusterMarkers, leadMarker } from "../../timeline/clusters";
import {
  GRAPH_HEIGHT,
  GRAPH_WIDTH,
  graphView,
  MAX_RULER_LABELS,
  type MetricKey,
  RULER_STEPS,
  rulerStep,
} from "../../timeline/graph";
import { markerLabel, markerStyle } from "../../timeline/markers";
import { type ViewingWindow, windowFraction } from "../../timeline/window";
import MarkerTimes from "./MarkerTimes.svelte";

interface Props {
  markers: readonly MarkerRow[];
  samples: readonly import("../../../types").SampleRow[];
  metric: MetricKey;
  window: ViewingWindow;
  /** The video's position, pushed by the player's rAF loop. */
  currentTimeS: number;
  onmetric: (metric: MetricKey) => void;
  onseek: (videoTimeS: number) => void;
  /** Click-to-seek and hold-to-scrub, shared with the in-player bar. */
  onscrub: (track: HTMLElement, clientX: number) => void;
  onscrubstart: (track: HTMLElement, e: PointerEvent) => void;
}

const {
  markers,
  samples,
  metric,
  window,
  currentTimeS,
  onmetric,
  onseek,
  onscrub,
  onscrubstart,
}: Props = $props();

let body = $state<HTMLElement>();
let tooltip = $state<HTMLElement>();
let bodyWidth = $state(0);

const view = $derived(graphView(samples, metric, window, windowFraction));
const playheadPct = $derived(windowFraction(currentTimeS, window) * 100);

// Clusters are measured in pixels, so they have to be recomputed when the
// element resizes. `review.ts` used a ResizeObserver for the same reason;
// `bind:clientWidth` is the same measurement with the observer built in.
const clusters = $derived(
  bodyWidth === 0
    ? []
    : clusterMarkers(
        markers,
        (m) => windowFraction(m.video_time_s, window) * bodyWidth,
        CLUSTER_PX,
      ),
);

const ruler = $derived.by(() => {
  const span = window.span;
  if (span <= 0) return { step: 0, labels: [] as number[] };
  const step = rulerStep(span, RULER_STEPS, MAX_RULER_LABELS);
  const labels: number[] = [];
  for (let t = 0; t <= span; t += step) labels.push(t);
  return { step, labels };
});

/** Which cluster's tooltip is showing, by index. */
let hovered = $state<number | null>(null);
let tooltipLeft = $state(0);
let scrollable = $state(false);

/**
 * Puts the tooltip over its glyph without letting it leave the timeline.
 *
 * `left` used to be the glyph's own percentage, which with
 * `translateX(-50%)` put half the box outside the track for any glyph near
 * either end, and the page grew sideways to contain it, which is what made
 * the window scrollable. Clamping needs the rendered width, so it cannot be
 * expressed in CSS.
 *
 * A tooltip wider than the timeline is pinned to the left rather than
 * centred (`max` before `min`), because the start of a marker list is the
 * part worth reading.
 */
function placeTooltip(percent: number) {
  const width = body?.clientWidth ?? 0;
  if (width === 0 || !tooltip) return;
  const half = tooltip.offsetWidth / 2;
  tooltipLeft = Math.max(half, Math.min((percent / 100) * width, width - half));
  // Only a tooltip that actually overflows takes the pointer. It overlaps
  // the top of the track, so making it hoverable unconditionally would put
  // a dead strip over the glyphs underneath it.
  scrollable = tooltip.scrollHeight > tooltip.clientHeight;
  if (scrollable) tooltip.scrollTop = 0;
}

function showTooltip(index: number, percent: number) {
  hovered = index;
  // Measured after the DOM has the content, which is what the old code got
  // by unhiding before measuring in the same handler.
  queueMicrotask(() => placeTooltip(percent));
}

function hideTooltip(e: MouseEvent) {
  // A scrollable tooltip has to survive the pointer moving into it, or its
  // scrollbar is unreachable: leaving the glyph is what normally dismisses
  // it, and the tooltip is not inside the glyph container.
  const into = e.relatedTarget;
  if (scrollable && into instanceof Node && tooltip?.contains(into)) return;
  hovered = null;
}
</script>

<div class="vod-timeline">
  <div class="timeline-head">
    <span class="timeline-metric-summary" class:hint={view.kind !== "curve"}>{view.summary}</span>
    <select
      class="timeline-metric-select"
      hidden={view.picker === "hidden"}
      disabled={view.picker === "disabled"}
      value={metric}
      onchange={(e) => onmetric((e.currentTarget as HTMLSelectElement).value as MetricKey)}
    >
      <option value="gold_diff">Gold diff</option>
      <option value="kill_diff">Kill diff</option>
      <option value="cs_diff">CS diff</option>
      <option value="none">No graph</option>
    </select>
  </div>

  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="timeline-body"
    bind:this={body}
    bind:clientWidth={bodyWidth}
    onpointerdown={(e) => {
      // A press on a glyph jumps precisely rather than seeking to the glyph's
      // pixel position, which is what `bindScrubbing`'s `intercept` did.
      if ((e.target as HTMLElement).closest("[data-cluster]")) return;
      onscrubstart(e.currentTarget as HTMLElement, e);
    }}
    onpointermove={(e) => onscrub(e.currentTarget as HTMLElement, e.clientX)}
  >
    {#if view.kind === "curve"}
      <svg
        class="timeline-graph"
        viewBox="0 0 {GRAPH_WIDTH} {GRAPH_HEIGHT}"
        preserveAspectRatio="none"
        aria-hidden="true"
      >
        <defs>
          <clipPath id="tl-clip-ahead"><rect x="0" y="0" width={GRAPH_WIDTH} height="50" /></clipPath>
          <clipPath id="tl-clip-behind"><rect x="0" y="50" width={GRAPH_WIDTH} height="50" /></clipPath>
        </defs>
        <path class="tl-area tl-area-ahead" d={view.area} clip-path="url(#tl-clip-ahead)" />
        <path class="tl-area tl-area-behind" d={view.area} clip-path="url(#tl-clip-behind)" />
        <line
          class="tl-baseline"
          x1="0"
          y1="50"
          x2={GRAPH_WIDTH}
          y2="50"
          vector-effect="non-scaling-stroke"
        />
        <path class="tl-line" d={view.line} vector-effect="non-scaling-stroke" />
      </svg>
    {/if}

    <div class="timeline-glyphs">
      {#each clusters as cluster, index (cluster[0].id)}
        {@const lead = leadMarker(cluster)}
        {@const style = markerStyle(lead)}
        {@const percent = windowFraction(clusterCentre(cluster), window) * 100}
        <button
          type="button"
          class="marker-glyph"
          data-cluster={index}
          style="left:{percent.toFixed(3)}%; --marker-color:{style.color}"
          aria-label={cluster.map((m) => `${markerLabel(m)} at ${formatTime(m.video_time_s)}`).join("; ")}
          onclick={() => onseek(cluster[0].video_time_s)}
          onmouseenter={() => showTooltip(index, percent)}
          onmouseleave={hideTooltip}
        >
          {style.icon}{#if cluster.length > 1}<span class="glyph-badge">{cluster.length}</span>{/if}
        </button>
      {/each}
    </div>

    <div class="timeline-ruler" style="--minor-gap:{((ruler.step / 4) / (window.span || 1)) * 100}%">
      {#each ruler.labels as t (t)}
        <span class="ruler-label" style="left:{((t / (window.span || 1)) * 100).toFixed(3)}%">
          {formatTime(t)}
        </span>
      {/each}
    </div>

    <div class="timeline-playhead" style="left:{playheadPct.toFixed(3)}%"></div>
  </div>

  <!--
    Payload strings carry other players' names. `review.ts` escaped them by
    hand here; interpolation does it now.
  -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="timeline-tooltip"
    bind:this={tooltip}
    hidden={hovered === null}
    data-scrollable={scrollable}
    style="left:{tooltipLeft}px"
    onmouseleave={hideTooltip}
  >
    {#if hovered !== null && clusters[hovered]}
      {#each clusters[hovered] as marker (marker.id)}
        <span class="tooltip-row">
          <span class="marker-icon">{markerStyle(marker).icon}</span>{markerLabel(marker)}
          <MarkerTimes {marker} />
        </span>
      {/each}
    {/if}
  </div>
</div>
