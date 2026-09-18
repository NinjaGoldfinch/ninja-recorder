<!--
  The library view - WS4 task 4.3.

  Replaces `library.ts` and the `#library-view` markup, which are both deleted.

  **`render()` is gone rather than ported.** The old module recomputed the
  visible rows and rebuilt the grid with `innerHTML` whenever anything changed,
  and had to defer that by hand while another view was showing (`pendingRender`)
  because rebuilding a hidden grid is wasted work and doing it as the user
  navigated back would yank the card they came from out from under them. Svelte
  does not render a component that is not mounted and keeps the elements it
  already has, so both problems and the machinery for them are gone.
-->

<script lang="ts">
import { call, hasDevCommands } from "../../../bridge";
import { openReview } from "../../../review";
import type { RecordingRow } from "../../../types";
import { fillInArt } from "../../stores/icons.svelte";
import {
  clearFilters,
  deleteRecording,
  library,
  refreshDiskUsage,
  refreshLibrary,
  rescanRecordings,
  togglePin,
} from "../../stores/library.svelte";
import Row from "./Row.svelte";
import StatsBar from "./StatsBar.svelte";
import Toolbar from "./Toolbar.svelte";

const rows = $derived(library.visible);
const filtering = $derived(library.filtersActive && library.rows.length > 0);

// Asked for, never configured. Resolves once per session; a row renders
// without the button until it answers, which is the same order the class
// toggle used to happen in.
let showInspect = $state(false);
void hasDevCommands().then((yes) => {
  showInspect = yes;
});

// Art is a second pass over whatever is currently visible. `library.ts` had
// to re-run this after every render because `innerHTML` threw the images
// away; here it re-runs when the row set changes, which is the only time
// there is anything new to fetch.
$effect(() => {
  void fillInArt(rows);
});
</script>

<StatsBar stats={library.stats} />

<Toolbar
  onrefresh={() => {
    // Both, as the button always has: the figure under "On disk" is free
    // space, which a refresh of the row set alone would leave stale.
    void refreshLibrary();
    void refreshDiskUsage();
  }}
  onrescan={rescanRecordings}
/>

{#if rows.length === 0}
  <!--
    "Nothing recorded yet" and "everything is filtered out" are different
    problems with different next steps. The first message was the only one
    there used to be, which read as data loss the moment a filter matched
    nothing.
  -->
  {#if filtering}
    <div class="empty-state">
      <p class="empty-title">No recordings match these filters</p>
      <p class="hint">
        {#if library.rows.length === 1}
          The one recording in the library does not match.
        {:else}
          {library.rows.length} recordings in the library &mdash; none of them match.
        {/if}
      </p>
      <button type="button" class="ghost" onclick={clearFilters}>Clear filters</button>
    </div>
  {:else}
    <div class="empty-state">
      <p class="empty-title">No recordings yet</p>
      <p class="hint">
        Recordings appear here once a game finishes &mdash; or any video file dropped
        into the recordings folder, via &ldquo;Rescan folder&rdquo; above.
      </p>
    </div>
  {/if}
{:else}
  <div class="vod-list" role="list">
    {#each rows as row (row.id)}
      <Row
        {row}
        {showInspect}
        onopen={openReview}
        onpin={(r: RecordingRow) => void togglePin(r)}
        ondelete={(r: RecordingRow) => void deleteRecording(r)}
        oninspect={(r: RecordingRow) =>
          void call("dev_open_portal", { recordingId: r.id }).catch((err) =>
            console.warn("dev portal unavailable:", err),
          )}
      />
    {/each}
  </div>
{/if}
