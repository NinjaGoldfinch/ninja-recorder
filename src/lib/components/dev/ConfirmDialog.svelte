<!--
  The portal's confirmation, for operations that write or destroy.

  `typeToConfirm` is reserved for the ones that cannot be undone: the button
  stays disabled until the exact text is typed, which is the difference between
  a click you meant and a click you were already making.
-->

<script lang="ts">
import type { ConfirmOptions } from "../../dev/ui-types";

let options = $state<ConfirmOptions | null>(null);
let typed = $state("");
let answer: ((ok: boolean) => void) | null = null;

const ready = $derived(!options?.typeToConfirm || typed === options.typeToConfirm);

/** Asks, and resolves with the answer. `App` hands this to callers. */
export function ask(next: ConfirmOptions): Promise<boolean> {
  options = next;
  typed = "";
  return new Promise<boolean>((resolve) => {
    answer = resolve;
  });
}

function close(ok: boolean) {
  const resolve = answer;
  answer = null;
  options = null;
  resolve?.(ok);
}

function onKey(e: KeyboardEvent) {
  if (e.key === "Escape") close(false);
  // Enter confirms only when the dialog is already satisfied, so a
  // type-to-confirm cannot be skipped by holding Enter.
  if (e.key === "Enter" && ready) close(true);
}
</script>

<svelte:window onkeydown={options ? onKey : undefined} />

{#if options}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="modal-backdrop"
    onclick={(e) => {
      // Only a press on the backdrop itself dismisses; one that started
      // inside the dialog and ended out here must not.
      if (e.target === e.currentTarget) close(false);
    }}
  >
    <div class="modal" role="dialog" aria-modal="true">
      <h2>{options.title}</h2>
      <p>{options.body}</p>
      {#if options.typeToConfirm}
        <label class="field">
          <span>Type <code>{options.typeToConfirm}</code> to confirm</span>
          <input type="text" autocomplete="off" spellcheck="false" bind:value={typed} />
        </label>
      {/if}
      <div class="row" style="margin-top:1rem">
        <button type="button" onclick={() => close(false)}>Cancel</button>
        <button type="button" class="danger" disabled={!ready} onclick={() => close(true)}>
          {options.confirmLabel ?? "Confirm"}
        </button>
      </div>
    </div>
  </div>
{/if}
