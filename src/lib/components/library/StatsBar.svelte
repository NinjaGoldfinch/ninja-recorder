<!--
  The four numbers above the library.

  Computed over the **filtered** rows so they track the filters, with a
  sub-label naming the total whenever a filter is narrowing things. Otherwise
  "100%" under Wins-only reads as a perfect record. `libraryStats` owns that
  rule and has the tests for it.
-->

<script lang="ts">
import { formatBytes, formatSpan } from "../../../format";
import type { LibraryStats } from "../../library/stats";

const { stats }: { stats: LibraryStats } = $props();
</script>

<section class="stats-bar" id="stats-bar">
  <div class="stat">
    <span class="stat-value">{stats.games}</span>
    <span class="stat-label">Games</span>
    <span class="stat-sub">{stats.gamesSub}</span>
  </div>
  <div class="stat">
    <span class="stat-value">{stats.winrate}</span>
    <span class="stat-label">Win rate</span>
    <span class="stat-sub">{stats.winrateSub}</span>
  </div>
  <div class="stat">
    <span class="stat-value">{formatSpan(stats.playtime)}</span>
    <span class="stat-label">Recorded</span>
    <span class="stat-sub">{stats.playtimeSub}</span>
  </div>
  <div class="stat">
    <span class="stat-value">{formatBytes(stats.diskBytes)}</span>
    <span class="stat-label">On disk</span>
    <span class="stat-sub">
      {#if stats.diskFreeBytes !== null}{formatBytes(stats.diskFreeBytes)} free{/if}
    </span>
  </div>
</section>
