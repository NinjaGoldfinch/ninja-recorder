<!--
  A player's line: K/D/A, the ratio, and CS with CS per minute.

  One component for both players, so ours and the opponent's are the same
  three lines in the same order and the pair reads as a comparison (#341).

  Deaths in their own colour, which is the one number on a row people look
  for first. Built from the three integers rather than by splitting
  `formatKda`'s string, so nothing here parses its own output; `formatKda`
  still owns the all-three-or-nothing rule, which is why the condition is on
  it.
-->

<script lang="ts">
import { formatKda, kdaRatio } from "../../../format";
import { csPerMinute } from "../../library/scoreboard";

interface Props {
  kills: number | null;
  deaths: number | null;
  assists: number | null;
  cs: number | null;
  /** The game's length. The same for both players. */
  durationS: number | null;
}

const { kills, deaths, assists, cs, durationS }: Props = $props();

const kda = $derived(formatKda(kills, deaths, assists));
const ratio = $derived(kdaRatio(kills, deaths, assists));
const perMinute = $derived(csPerMinute(cs, durationS));
</script>

<span class="vod-cell vod-stats">
  <span class="vod-value vod-kda">
    {#if kda === null}
      <span class="vod-missing">&mdash;</span>
    {:else}
      {kills}<span class="vod-slash">/</span><span class="vod-deaths">{deaths}</span><span
        class="vod-slash">/</span
      >{assists}
    {/if}
  </span>
  <span class="vod-sub">{#if ratio}{ratio}{:else}&nbsp;{/if}</span>
  <span class="vod-sub">
    {#if cs === null}&nbsp;{:else}{cs} CS{#if perMinute}&nbsp;&middot; {perMinute}{/if}{/if}
  </span>
</span>
