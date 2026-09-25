<!--
  Review, pin, delete and (behind devtools) inspect.

  Delete is a **two-step on the button itself** rather than a modal: the first
  click arms it, the second deletes, and it disarms itself after four seconds.
  Cheaper than a dialog, and it keeps the destructive action next to the thing
  it destroys.

  The arming lives here, per row, rather than in the library's state. In
  `library.ts` it could not: the grid was rebuilt with `innerHTML` on every
  render, so the armed button was a node that kept being thrown away, and one
  module-level `armedForDelete` plus a `querySelector` to find the button again
  was the only way to survive that. Svelte keeps the element, so the flag can
  live where the button does.
-->

<script lang="ts">
import type { RecordingRow } from "../../../types";

interface Props {
  row: RecordingRow;
  onreview: (row: RecordingRow) => void;
  onpin: (row: RecordingRow) => void;
  ondelete: (row: RecordingRow) => void;
  oninspect: (row: RecordingRow) => void;
  showInspect: boolean;
}

const { row, onreview, onpin, ondelete, oninspect, showInspect }: Props = $props();

let armed = $state(false);
let armTimer: ReturnType<typeof setTimeout> | undefined;

function onDelete() {
  if (armed) {
    disarm();
    ondelete(row);
    return;
  }
  armed = true;
  clearTimeout(armTimer);
  armTimer = setTimeout(disarm, 4000);
}

function disarm() {
  clearTimeout(armTimer);
  armed = false;
}

// A row that scrolls out, gets filtered away or is re-sorted takes its timer
// with it. Without this the callback still fires, on a component that is no
// longer rendering anything.
$effect(() => () => clearTimeout(armTimer));
</script>

<!--
  Clicks here must not fall through to opening the VOD. `stopPropagation` on
  the group rather than on each button: `library.ts` did this by checking
  `closest(".vod-actions")` in a delegated handler on the grid, and the
  equivalent here is to stop it at the same boundary.
-->
<span
  class="vod-actions"
  onclick={(e) => e.stopPropagation()}
  onkeydown={(e) => e.stopPropagation()}
  role="presentation"
>
  <!--
    The WS9 review form: ratings, objectives, takeaways. Separate from opening
    the row, which plays the VOD; P1 puts the two side by side.
  -->
  <button
    class="icon-btn review-btn"
    type="button"
    aria-label="Review this game"
    title="Review this game"
    onclick={() => onreview(row)}>📝</button
  >
  <button
    class="icon-btn pin-btn"
    class:pinned={row.pinned}
    type="button"
    aria-pressed={row.pinned}
    title={row.pinned ? "Unpin" : "Pin (exempt from disk retention)"}
    onclick={() => onpin(row)}>📌</button
  >
  <button
    class="icon-btn danger"
    class:armed
    type="button"
    aria-label="Delete recording"
    title="Delete recording"
    onclick={onDelete}>{armed ? "Delete?" : "🗑"}</button
  >
  <!--
    Devtools only, revealed the same way the portal button is: asked for, never
    configured. `library.ts` rendered it hidden and toggled a class after the
    probe answered, because the markup had to have one shape. Here the probe's
    answer is a prop, so the button simply is not rendered.
  -->
  {#if showInspect}
    <button
      class="icon-btn dev-only"
      type="button"
      aria-label="Inspect in the dev portal"
      title="Inspect in the dev portal"
      onclick={() => oninspect(row)}>🔎</button
    >
  {/if}
</span>
