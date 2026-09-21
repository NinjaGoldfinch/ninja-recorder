<!--
  A definition list.

  An absent value renders as a dash in the hint style rather than an empty
  cell, so a row that has nothing to say still occupies its line and the list
  does not change shape between renders.
-->

<script lang="ts">
import { MISSING } from "../../dev/format";

interface Props {
  pairs: Array<[string, string | null | undefined]>;
  /** Skips the monospace treatment, for values that are prose. */
  plain?: boolean;
}

const { pairs, plain = false }: Props = $props();

const missing = (v: string | null | undefined) => v === null || v === undefined || v === "";
</script>

<dl class="kv">
  {#each pairs as [key, value] (key)}
    <dt>{key}</dt>
    <dd class:plain>
      {#if missing(value)}<span class="hint">{MISSING}</span>{:else}{value}{/if}
    </dd>
  {/each}
</dl>
