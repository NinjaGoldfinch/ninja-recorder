<!--
  The transient message.

  Replaced `#status-msg`, which sat above the library and carried both progress
  and failures. The failures are why it could not simply be deleted with the
  layout it lived in.
-->

<script lang="ts">
import { toastState } from "../../stores/toast.svelte";
</script>

{#if toastState.message !== null}
  <div class="toast" role="alert" data-kind={toastState.kind}>{toastState.message}</div>
{/if}
