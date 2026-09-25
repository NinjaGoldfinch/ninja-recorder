<!--
  What the last recording lost to a capture failure (#10): the daemon's
  `captureProblems` event, in words (`library/problems.ts`).

  Beside the recorder strip and styled like it, because it is the same kind of
  thing: true of the whole window, whichever view is showing. It differs in one
  way, which is that it does not clear itself, so it can be dismissed.

  **The text is untrusted.** It carries what a Windows call said, and it is
  interpolated as text, never `{@html}`.
-->

<script lang="ts">
import { captureNotice, dismissCaptureNotice } from "../../stores/capture.svelte";
</script>

{#if captureNotice.notice}
  <div class="capture-strip" role="status" aria-live="polite" data-kind={captureNotice.notice.kind}>
    <span class="capture-strip-text">{captureNotice.notice.text}</span>
    <button type="button" class="capture-strip-dismiss" onclick={dismissCaptureNotice}>Dismiss</button>
  </div>
{/if}
