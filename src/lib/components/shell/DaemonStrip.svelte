<!--
  The recorder's own state, not the app's - WS3.8.

  **A strip, not a toast.** A toast announces something that happened and then
  goes away; this is a state, true until it stops being true, and it clears
  itself when the connection comes back rather than on a timer. Anything that
  disappeared on its own while the daemon was still missing would be worse than
  saying nothing.

  Above `main` rather than inside a view, because it is true of the whole
  window regardless of which view is showing.
-->

<script lang="ts">
import { daemon } from "../../stores/daemon.svelte";
</script>

{#if daemon.strip}
  <div class="daemon-strip" role="status" aria-live="polite" data-kind={daemon.strip.kind}>
    {daemon.strip.text}
  </div>
{/if}
