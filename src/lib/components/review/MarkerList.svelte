<!--
  Every marker, in order, under the player.

  **Payload strings carry other players' names**, which is why `review.ts`
  escaped them by hand here. Default interpolation replaces that.
-->

<script lang="ts">
import type { MarkerRow } from "../../../types";
import { markerLabel, markerStyle } from "../../timeline/markers";
import MarkerTimes from "./MarkerTimes.svelte";

interface Props {
  markers: readonly MarkerRow[];
  /**
   * The subset naming moments past the end of the file, which only a
   * recording whose finalize never ran has any of.
   *
   * Listed with the rest, because the events happened and this is the only
   * record that they did, but marked and not seekable: there is nothing at
   * that position to seek to.
   */
  beyond?: readonly MarkerRow[];
  onseek: (videoTimeS: number) => void;
}

const { markers, beyond = [], onseek }: Props = $props();

const outside = $derived(new Set(beyond.map((m) => m.id)));
</script>

<ul class="marker-list">
  {#if markers.length === 0}
    <li class="hint">No markers recorded for this game.</li>
  {:else}
    {#each markers as marker (marker.id)}
      <!--
        `review.ts` put a `data-time` on each row and read it back in a
        delegated click handler on the list. The seek is a callback now, so
        nothing has to parse its own attribute.

        **Click only, no keyboard, and that is the port being faithful rather
        than the port being careless.** These rows carried no `tabindex` and no
        key handler before, so adding them here would be a behaviour change in
        a task whose exit criterion is that a review session is identical. The
        gap is real and worth its own issue, alongside the same question about
        a library row.
      -->
      {@const unreachable = outside.has(marker.id)}
      <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
      <!-- svelte-ignore a11y_click_events_have_key_events -->
      <li
        class:beyond-footage={unreachable}
        style="--marker-color:{markerStyle(marker).color}"
        title={unreachable ? "This recording ends before this happened" : undefined}
        onclick={() => {
          // Nothing to seek to: the file does not reach this moment.
          if (!unreachable) onseek(marker.video_time_s);
        }}
      >
        <span class="marker-icon">{markerStyle(marker).icon}</span>
        <span class="marker-label">{markerLabel(marker)}</span>
        <MarkerTimes {marker} />
      </li>
    {/each}
  {/if}
</ul>

{#if beyond.length > 0}
  <p class="hint marker-list-note">
    <strong>
      {beyond.length}
      {beyond.length === 1 ? "marker names a moment" : "markers name moments"} after this recording
      ends,
    </strong>
    so {beyond.length === 1 ? "it is" : "they are"} listed but not on the timeline. This recording
    was never finalized, which is what leaves its marker times approximate: the events are real and
    the times they are drawn at are the ones the last poll guessed.
  </p>
{/if}
