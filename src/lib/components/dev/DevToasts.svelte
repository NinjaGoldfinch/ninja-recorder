<!--
  Where the portal's toasts land.

  **Inside `.dev-root`, not on `document.body`.** Every token (`--surface`,
  `--border`, …) and every control style is scoped to `.dev-root`, so an
  overlay parented to the body renders with a transparent background and
  unstyled buttons. `position: fixed` still resolves against the viewport from
  in here, because `.dev-root` is a plain grid with no transform or filter.
-->

<script lang="ts">
import { devToasts } from "../../stores/devToast.svelte";
</script>

{#if devToasts.items.length > 0}
  <div class="toast-stack">
    {#each devToasts.items as item (item.id)}
      <div class="toast {item.tone}">{item.message}</div>
    {/each}
  </div>
{/if}
