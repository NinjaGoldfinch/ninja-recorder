<!--
  Every marker, in order: the Events tab of the review rail (#326).

  A filter narrows it to fights, deaths or objectives; a burst of the same
  event collapses into one row ("Voidgrubs ×3", `timeline/events.ts`); and the
  row the playhead has most recently passed is lit and kept in view while the
  video plays, unless the pointer is over the list, where it would scroll the
  list out from under the person reading it.

  **Payload strings carry other players' names**, which is why `review.ts`
  escaped them by hand here. Default interpolation replaces that.
-->

<script lang="ts">
import { formatTime } from "../../../format";
import type { MarkerRow } from "../../../types";
import type { NoteKind } from "../../contract/types";
import { noteStyle, type PlacedNote } from "../../review/notes";
import {
  currentItem,
  type EventFilter,
  eventRows,
  listItems,
  matchesFilter,
} from "../../timeline/events";
import { markerStyle } from "../../timeline/markers";
import MarkerTimes from "./MarkerTimes.svelte";
import NoteEditor from "./NoteEditor.svelte";

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
  /** Where the player is, to light the current row. Negative for nowhere. */
  currentTimeS?: number;
  /** The game's timed notes, placed in the recording (#258). */
  notes?: readonly PlacedNote[];
  onnoteupdate?: (noteId: number, kind: NoteKind, body: string) => Promise<boolean>;
  onnotedelete?: (noteId: number) => void;
}

const {
  markers,
  beyond = [],
  onseek,
  currentTimeS = -1,
  notes = [],
  onnoteupdate,
  onnotedelete,
}: Props = $props();

/** The note being rewritten in place, if any. */
let editing = $state<number | null>(null);

const FILTERS: { value: EventFilter; label: string }[] = [
  { value: "all", label: "All" },
  { value: "kills", label: "Fights" },
  { value: "deaths", label: "Deaths" },
  { value: "objectives", label: "Objectives" },
  { value: "notes", label: "Notes" },
];

let filter = $state<EventFilter>("all");
let list = $state<HTMLUListElement>();
let reading = false;

const outside = $derived(new Set(beyond.map((m) => m.id)));
const rows = $derived(eventRows(markers, outside, filter));
const items = $derived(listItems(rows, notes, filter));
const current = $derived(currentItem(items, currentTimeS));
const counts = $derived(
  Object.fromEntries(
    FILTERS.map((f) => [
      f.value,
      f.value === "notes"
        ? notes.length
        : markers.filter((m) => matchesFilter(m.kind, f.value)).length +
          (f.value === "all" ? notes.length : 0),
    ]),
  ) as Record<EventFilter, number>,
);

// Follows the playhead by scrolling the lit row into view, but only when
// the row changes, and never while the pointer is over the list.
$effect(() => {
  const index = current;
  if (index < 0 || reading || !list) return;
  const row = list.children[index] as HTMLElement | undefined;
  row?.scrollIntoView?.({ block: "nearest" });
});
</script>

{#if markers.length > 0 || notes.length > 0}
  <div class="event-filters" role="group" aria-label="Show">
    {#each FILTERS as f (f.value)}
      <button
        type="button"
        class="event-filter"
        aria-pressed={filter === f.value}
        disabled={counts[f.value] === 0}
        onclick={() => (filter = f.value)}
        >{f.label} <span class="event-filter-count">{counts[f.value]}</span></button
      >
    {/each}
  </div>
{/if}

<ul
  class="marker-list"
  bind:this={list}
  onpointerenter={() => (reading = true)}
  onpointerleave={() => (reading = false)}
>
  {#if markers.length === 0 && notes.length === 0}
    <li class="hint">No markers recorded for this game. Press n to add a note.</li>
  {:else if items.length === 0}
    <li class="hint">Nothing of this kind in this game.</li>
  {:else}
    {#each items as item, i (item.type === "event" ? `e${item.row.marker.id}` : `n${item.placed.note.id}`)}
      {#if item.type === "event"}
        <!--
          `review.ts` put a `data-time` on each row and read it back in a
          delegated click handler on the list. The seek is a callback now, so
          nothing has to parse its own attribute.

          **Click only, no keyboard, and that is the port being faithful rather
          than the port being careless.** These rows carried no `tabindex` and
          no key handler before, so adding them here would be a behaviour
          change in a task whose exit criterion is that a review session is
          identical. The gap is real and worth its own issue, alongside the
          same question about a library row.
        -->
        {@const row = item.row}
        {@const marker = row.marker}
        <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
        <!-- svelte-ignore a11y_click_events_have_key_events -->
        <li
          class:beyond-footage={row.beyond}
          class:current={i === current}
          aria-current={i === current ? "true" : undefined}
          style="--marker-color:{markerStyle(marker).color}"
          title={row.beyond ? "This recording ends before this happened" : undefined}
          onclick={() => {
            if (!row.beyond) onseek(marker.video_time_s);
          }}
        >
          <span class="marker-icon">{markerStyle(marker).icon}</span>
          <span class="marker-label"
            >{row.label}{#if row.count > 1}<span class="marker-count">{` ×${row.count}`}</span
              >{/if}</span
          >
          <MarkerTimes {marker} />
        </li>
      {:else}
        <!--
          A note is the user's words, so it is edited here, in place, with the
          same editor `n` opens. Its text is interpolated, never `{@html}`.
        -->
        {@const note = item.placed.note}
        {@const style = noteStyle(note.kind)}
        {#if editing === note.id}
          <li class="note-row editing">
            <NoteEditor
              label="Note at {formatTime(note.ts_ms / 1000)}"
              kind={note.kind}
              body={note.body}
              onsave={async (kind, body) => {
                if ((await onnoteupdate?.(note.id, kind, body)) ?? false) editing = null;
              }}
              oncancel={() => (editing = null)}
            />
          </li>
        {:else}
          <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
          <!-- svelte-ignore a11y_click_events_have_key_events -->
          <li
            class="note-row"
            class:current={i === current}
            aria-current={i === current ? "true" : undefined}
            style="--marker-color:{style.color}"
            onclick={() => onseek(item.placed.videoTimeS)}
          >
            <span class="marker-icon" title={style.label}>{style.icon}</span>
            <span class="note-body">{note.body}</span>
            <!-- The same two clocks, in the same columns, as an event's. -->
            <span class="marker-times">
              <span class="marker-game-time" title="Game clock">{formatTime(note.ts_ms / 1000)}</span>
              <span class="hint" title="Position in the recording"
                >{formatTime(item.placed.videoTimeS)}</span
              >
            </span>
            <span class="note-actions">
              <button
                type="button"
                class="icon-btn"
                aria-label="Edit note"
                title="Edit"
                onclick={(e) => {
                  e.stopPropagation();
                  editing = note.id;
                }}>✎</button
              >
              <button
                type="button"
                class="icon-btn danger"
                aria-label="Delete note"
                title="Delete"
                onclick={(e) => {
                  e.stopPropagation();
                  onnotedelete?.(note.id);
                }}>✕</button
              >
            </span>
          </li>
        {/if}
      {/if}
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
