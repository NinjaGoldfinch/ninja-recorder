<!--
  What changed, from `latest.json`.

  **Never markup.** The manifest is fetched over HTTPS but is *not* covered by
  the update signature - only the installer it points at is - so everything
  rendered here is remote text this app did not write. `update.ts` built it as
  DOM nodes with `createElement` and `textContent` for exactly that reason;
  interpolating a parsed structure gets the same guarantee from the template.

  **An `{@html}` here would throw it away.** `notes.test.ts` covers the parser
  and `Update.test.ts` covers this component, and both would fail.
-->

<script lang="ts">
import { parseNotes } from "../../settings/notes";

const { notes }: { notes: string | null } = $props();
const blocks = $derived(parseNotes(notes));
</script>

{#if blocks.length > 0}
  <div class="update-notes">
    {#each blocks as block, i (i)}
      {#if block.kind === "list"}
        <ul>
          {#each block.items as item, j (j)}
            <li>
              {#each item as span, k (k)}
                {#if span.strong}<strong>{span.text}</strong>{:else}{span.text}{/if}
              {/each}
            </li>
          {/each}
        </ul>
      {:else}
        <p>
          {#each block.spans as span, j (j)}
            {#if span.strong}<strong>{span.text}</strong>{:else}{span.text}{/if}
          {/each}
        </p>
      {/if}
    {/each}
  </div>
{/if}
