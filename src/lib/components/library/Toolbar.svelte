<!--
  Search, the three derived filters, outcome, pinned-only, sort, and the two
  folder actions.

  The three derived filters used to ship with only their "All" option in the
  markup, and `library.ts` filled the rest from the rows actually in the
  library, because patch cannot be enumerated ahead of time and the other two
  would otherwise offer choices that match nothing. They are still built from
  the rows; what has gone is the hand-written `select.remove(1)` loop that
  rebuilt the `<option>` list in place.
-->

<script lang="ts">
import { ANY } from "../../library/filters";
import { library } from "../../stores/library.svelte";

const { onrefresh, onrescan }: { onrefresh: () => void; onrescan: () => void } = $props();

const controls = library.controls;
let rescanning = $state(false);

async function rescan() {
  rescanning = true;
  try {
    await onrescan();
  } finally {
    rescanning = false;
  }
}
</script>

<div class="toolbar">
  <div class="search-field">
    <svg
      viewBox="0 0 24 24"
      width="16"
      height="16"
      fill="none"
      stroke="currentColor"
      stroke-width="1.8"
      aria-hidden="true"
    >
      <circle cx="11" cy="11" r="6.5" />
      <path d="m16 16 4.5 4.5" stroke-linecap="round" />
    </svg>
    <input
      type="search"
      placeholder="Search champion&hellip;"
      aria-label="Search champion"
      bind:value={controls.champion}
    />
  </div>

  {#each library.facets as facet (facet.id)}
    <select
      aria-label={facet.ariaLabel}
      disabled={facet.disabled}
      bind:value={controls[facet.id]}
    >
      {#each facet.options as option (option.value)}
        <!-- The label is a text node, never markup: `game_mode` reaches it
             straight from Live Client Data. -->
        <option value={option.value}>{option.label}</option>
      {/each}
    </select>
  {/each}

  <select aria-label="Filter by result" bind:value={controls.outcome}>
    <option value={ANY}>All results</option>
    <option value="wins">Wins only</option>
    <option value="losses">Losses only</option>
  </select>

  <label class="checkbox-label">
    <input type="checkbox" bind:checked={controls.pinnedOnly} />
    Pinned only
  </label>

  <select aria-label="Sort order" bind:value={controls.sort}>
    <option value="newest">Newest first</option>
    <option value="oldest">Oldest first</option>
    <option value="longest">Longest first</option>
    <option value="champion">Champion (A&ndash;Z)</option>
  </select>

  <span class="toolbar-spacer"></span>

  <button type="button" class="ghost" disabled={rescanning} onclick={rescan}>
    Rescan folder
  </button>
  <button
    type="button"
    class="icon-btn ghost"
    aria-label="Refresh library"
    title="Refresh library"
    onclick={onrefresh}
  >
    <svg
      viewBox="0 0 24 24"
      width="16"
      height="16"
      fill="none"
      stroke="currentColor"
      stroke-width="1.8"
      stroke-linecap="round"
    >
      <path d="M20 11a8 8 0 1 0-.7 4.2" />
      <path d="M20 5v6h-6" />
    </svg>
  </button>
</div>

<!--
  The one control this file deliberately does not own is "Clear filters": it
  lives in the no-matches empty state, which is the only place it is useful.
-->
