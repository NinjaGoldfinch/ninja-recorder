<!--
  The lane matchup: us against the one opponent who played our position.

  **It replaced the ten team portraits**, and the trade is deliberate. Ten
  champions told you who was in the game; one tells you who you actually played
  against, which is the thing a person is reconstructing when they scan a
  library. "The Darius game" is a matchup, not a lobby. The other nine are
  still in `scoreboard_json` for anything that wants them.

  Drawn at a fixed width whatever it holds, so the columns either side of it
  land in the same place on every row.
-->

<script lang="ts">
import type { RecordingRow } from "../../../types";
import { laneOpponent } from "../../library/scoreboard";
import Slot from "./Slot.svelte";

const { row }: { row: RecordingRow } = $props();

const them = $derived(laneOpponent(row));

// Six plus the trinket, padded, exactly as ours is.
const items = $derived(
  them === null
    ? []
    : [
        ...them.items.slice(0, 7).map((id) => ({ key: id })),
        ...Array(Math.max(0, 7 - them.items.length)).fill(null),
      ],
);
</script>

{#if them === null}
  <!--
    No opponent is said in words, not drawn as an empty skeleton. A row of
    blank boxes beside a "vs" reads as art that failed to load, which is a bug
    report waiting to happen, where "no matchup" reads as what it is. The block
    keeps its width either way, so the columns do not move.
  -->
  <span class="vod-versus" data-unknown="true">
    <span class="vod-versus-label" aria-hidden="true">vs</span>
    <span class="vod-sub">No matchup recorded</span>
  </span>
{:else}
  <span class="vod-versus">
    <span class="vod-versus-label" aria-hidden="true">vs</span>
    <Slot kind="champion" key={them.champion} title={them.champion} extra="vod-versus-portrait" />
    <span class="vod-cell vod-versus-line">
      <!-- Their line, in the same shape ours takes so the two read as a pair. -->
      <span class="vod-value vod-kda">
        {them.kills}
        <span class="vod-slash">/</span>
        <span class="vod-deaths">{them.deaths}</span>
        <span class="vod-slash">/</span>
        {them.assists}
      </span>
      <span class="vod-sub">{them.cs} cs</span>
    </span>
    <span class="vod-items" aria-hidden="true">
      {#each items as item, i (i)}
        {#if item === null}
          <Slot />
        {:else}
          <Slot kind="item" key={item.key} title={`Item ${item.key}`} />
        {/if}
      {/each}
    </span>
  </span>
{/if}
