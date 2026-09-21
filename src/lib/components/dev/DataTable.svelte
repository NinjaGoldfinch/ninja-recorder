<!--
  A grid of rows.

  **Every cell is text.** These render database rows and command output, which
  is backend data this frontend did not write, and `ui.ts` escaped each cell
  and each `title` by hand for exactly that reason.

  `null` is rendered as the word rather than as an empty cell, because in a
  debug table the difference between "no value" and "empty string" is usually
  the thing being looked for.
-->

<script lang="ts">
interface Props {
  columns: string[];
  rows: unknown[][];
  /** Marks rows clickable and reports which one was pressed. */
  onrow?: (index: number) => void;
  /** Right-aligns these columns and renders them tabular-nums. */
  numericColumns?: ReadonlySet<string>;
  emptyMessage?: string;
}

const {
  columns,
  rows,
  onrow,
  numericColumns = new Set<string>(),
  emptyMessage = "No rows.",
}: Props = $props();

function cellText(cell: unknown): string {
  return typeof cell === "object" ? JSON.stringify(cell) : String(cell);
}
</script>

{#if rows.length === 0}
  <p class="hint">{emptyMessage}</p>
{:else}
  <div class="table-wrap">
    <table class="grid">
      <thead>
        <tr>
          {#each columns as column (column)}
            <th>{column}</th>
          {/each}
        </tr>
      </thead>
      <tbody>
        {#each rows as row, i (i)}
          <!--
            `ui.ts` stamped `data-row-index` and read it back in a delegated
            handler on the table. The index is closed over now, so nothing
            parses its own attribute.
          -->
          <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
          <!-- svelte-ignore a11y_click_events_have_key_events -->
          <tr class:clickable={!!onrow} onclick={() => onrow?.(i)}>
            {#each row as cell, j (j)}
              {#if cell === null || cell === undefined}
                <td class="null">null</td>
              {:else}
                <td class:num={numericColumns.has(columns[j])} title={cellText(cell)}>
                  {cellText(cell)}
                </td>
              {/if}
            {/each}
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
{/if}
