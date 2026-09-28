<!--
  The right-hand rail beside the player (WS9 P1): the review form and the
  event list, one tab each.

  **Both panels stay mounted and are hidden, not destroyed**, and so does the
  rail itself in theatre mode. The form keeps half-typed text that does not
  parse yet, and the list keeps its scroll position; an `{#if}` would throw
  both away every time the tab changed or the rail was folded.
-->

<script lang="ts">
import type { MarkerRow } from "../../../types";
import type { Note } from "../../contract/types";
import type { PlacedNote } from "../../review/notes";
import { deleteNote, gameReview } from "../../stores/gameReview.svelte";
import ReviewForm from "../reviewform/ReviewForm.svelte";
import MarkerList from "./MarkerList.svelte";

interface Props {
  /** False in theatre mode: the rail is folded away and the player widens. */
  open: boolean;
  markers: readonly MarkerRow[];
  beyond: readonly MarkerRow[];
  onseek: (videoTimeS: number) => void;
  /** Where the player is, for the Events tab's current row. */
  currentTimeS: number;
  /** The game clock at the playhead, for the form's clock button. */
  gameClockNow: () => number | null;
  /** The player's "note at the playhead", for the Events tab's note button. */
  onstamp: () => void;
  /** Opens a note in the player's editor: the Events tab's edit button. */
  onnoteedit: (note: Note) => void;
  /** The game's timed notes, placed in the recording, for the Events tab. */
  notes: readonly PlacedNote[];
}

const {
  open,
  markers,
  beyond,
  onseek,
  currentTimeS,
  gameClockNow,
  onstamp,
  onnoteedit,
  notes,
}: Props = $props();

type Tab = "review" | "events";
let tab = $state<Tab>("review");

const STATUS_COPY = {
  saved: "Saved",
  unsaved: "Unsaved changes",
  saving: "Saving…",
  error: "Not saved",
} as const;
</script>

<aside class="review-rail" aria-label="Review" hidden={!open}>
  <div class="rail-tabs" role="tablist">
    <button
      type="button"
      role="tab"
      id="rail-tab-review"
      aria-controls="rail-panel-review"
      aria-selected={tab === "review"}
      onclick={() => (tab = "review")}>Review</button
    >
    <button
      type="button"
      role="tab"
      id="rail-tab-events"
      aria-controls="rail-panel-events"
      aria-selected={tab === "events"}
      onclick={() => (tab = "events")}
      >Events <span class="rail-count">{markers.length}</span></button
    >
    {#if gameReview.current}
      <span class="save-status" data-status={gameReview.status} role="status" aria-live="polite"
        >{STATUS_COPY[gameReview.status]}</span
      >
    {/if}
  </div>

  <div
    class="rail-panel"
    role="tabpanel"
    id="rail-panel-review"
    aria-labelledby="rail-tab-review"
    hidden={tab !== "review"}
  >
    <ReviewForm {gameClockNow} />
  </div>

  <div
    class="rail-panel rail-events"
    role="tabpanel"
    id="rail-panel-events"
    aria-labelledby="rail-tab-events"
    hidden={tab !== "events"}
  >
    <MarkerList
      {markers}
      {beyond}
      {onseek}
      {currentTimeS}
      {notes}
      {onnoteedit}
      {onstamp}
      onnotedelete={(id) => void deleteNote(id)}
    />
  </div>
</aside>
