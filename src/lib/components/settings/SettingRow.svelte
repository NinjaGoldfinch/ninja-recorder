<!--
  The label-plus-hint-plus-control shape every setting uses.

  A component rather than repeated markup because the hint is the part most
  likely to be forgotten, and a row without one reads as a control with no
  explanation. The control goes in the default slot.
-->

<script lang="ts">
import type { Snippet } from "svelte";

interface Props {
  label: string;
  /** The short explanation under the label. Optional: a few rows carry their
   *  hint as richer markup and pass `copy` instead. */
  hint?: string | null;
  /** Replaces `hint` for the rows that need a list or emphasis in it. */
  copy?: Snippet;
  children: Snippet;
}

const { label, hint = null, copy, children }: Props = $props();
</script>

<div class="setting-row">
  <div class="setting-copy">
    <span class="setting-label">{label}</span>
    {#if copy}{@render copy()}{:else if hint}<p class="setting-hint">{hint}</p>{/if}
  </div>
  {@render children()}
</div>
