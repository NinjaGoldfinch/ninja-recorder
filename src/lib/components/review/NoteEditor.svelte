<!--
  Writing or rewriting one timed note: a kind, picked from a dropdown, and
  the text (#258).

  **One key to remember.** `n` opens this at the playhead with the text box
  focused and the kind on "Note"; the dropdown beside it is the only other
  control. The spec's per-kind keys (M, G, ?) were dropped for this: M was
  already mute, and one key plus a picker is less to learn than four.

  Enter saves and Shift+Enter starts a new line, in the text box and on the
  kind dropdown alike: picking a kind leaves the focus on the dropdown, and
  Enter there did nothing, so the note could not be saved from the keyboard
  without tabbing back. Enter on Cancel or Save still presses that button.
  Escape cancels. Every key
  pressed in here stops at this element, so typing never reaches the player's
  hotkeys (`review/hotkeys.ts`, on the document), and `m` in a note is a
  letter rather than a mute.

  The body is plain text (#251), rendered wherever it is shown by Svelte's
  text interpolation. No `{@html}`.
-->

<script lang="ts">
import { onMount, untrack } from "svelte";
import type { NoteKind } from "../../contract/types";
import { NOTE_KINDS, noteStyle } from "../../review/notes";

interface Props {
  /** Where the note is, as the player shows it: "Note at 6:36". */
  label: string;
  kind?: NoteKind;
  body?: string;
  onsave: (kind: NoteKind, body: string) => void | Promise<unknown>;
  oncancel: () => void;
}

const props: Props = $props();

// Seeded once from the props, deliberately (`untrack`): this is an editor,
// and what the person types is the state from here on.
let kind = $state<NoteKind>(untrack(() => props.kind ?? "note"));
let body = $state(untrack(() => props.body ?? ""));
let saving = $state(false);
let text = $state<HTMLTextAreaElement>();
let picker = $state<HTMLSelectElement>();

const empty = $derived(body.trim() === "");

onMount(() => {
  text?.focus();
  // The cursor after what is there, for an edit.
  text?.setSelectionRange(body.length, body.length);
});

async function save() {
  if (empty || saving) return;
  saving = true;
  try {
    await props.onsave(kind, body.trim());
  } finally {
    saving = false;
  }
}

function onKey(e: KeyboardEvent) {
  e.stopPropagation();
  if (e.key === "Escape") {
    e.preventDefault();
    props.oncancel();
  } else if (e.key === "Enter" && !e.shiftKey && (e.target === text || e.target === picker)) {
    e.preventDefault();
    void save();
  }
}
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div class="note-editor" role="group" aria-label={props.label} onkeydown={onKey}>
  <div class="note-editor-head">
    <span class="note-editor-label">{props.label}</span>
    <select
      class="note-editor-kind"
      aria-label="Kind"
      bind:this={picker}
      bind:value={kind}
      style="--note-color:{noteStyle(kind).color}"
    >
      {#each NOTE_KINDS as k (k)}
        <option value={k}>{noteStyle(k).icon} {noteStyle(k).label}</option>
      {/each}
    </select>
  </div>
  <textarea
    class="note-editor-text"
    bind:this={text}
    bind:value={body}
    rows="2"
    placeholder="What happened here?"
    aria-label="Note"
  ></textarea>
  <div class="note-editor-actions">
    <span class="hint">Enter saves &middot; Shift+Enter new line &middot; Esc cancels</span>
    <button type="button" class="ghost" onclick={props.oncancel}>Cancel</button>
    <button type="button" class="primary" disabled={empty || saving} onclick={() => void save()}>
      Save
    </button>
  </div>
</div>
