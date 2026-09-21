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
  onseek: (videoTimeS: number) => void;
}

const { markers, onseek }: Props = $props();
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
      <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
      <!-- svelte-ignore a11y_click_events_have_key_events -->
      <li
        style="--marker-color:{markerStyle(marker).color}"
        onclick={() => onseek(marker.video_time_s)}
      >
        <span class="marker-icon">{markerStyle(marker).icon}</span>
        <span class="marker-label">{markerLabel(marker)}</span>
        <MarkerTimes {marker} />
      </li>
    {/each}
  {/if}
</ul>
