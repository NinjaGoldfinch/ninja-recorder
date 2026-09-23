<!--
  Settings → Advanced: which capture backend records. WS1.7, #11.

  Every choice the daemon knows about is listed, and one this build cannot
  construct is shown disabled with the daemon's reason rather than left out:
  the own backend exists as a choice before it exists as code, and a missing
  option would say nothing about why. The reasons and the live backend's name
  come from the daemon, and are interpolated, never rendered as markup.

  Rendered only in a devtools build until WS1.6 (`Settings.svelte` holds the
  gate); WS1.6 un-hides it along with flipping the default.
-->

<script lang="ts">
import {
  APPLIES_WHEN,
  BACKEND_LABELS,
  refusalNote,
  unavailableNotes,
} from "../../settings/capture";
import { saveCaptureBackend, settings } from "../../stores/settings.svelte";
import SettingRow from "./SettingRow.svelte";

const status = $derived(settings.captureBackend);
const notes = $derived(status ? unavailableNotes(status) : []);
const refusal = $derived(status ? refusalNote(status) : null);
</script>

<section class="settings-group">
  <h3>Advanced</h3>

  <SettingRow label="Capture backend">
    {#snippet copy()}
      <p class="setting-hint">
        Which engine records your games. libobs is the one every recording so
        far was made with; the own backend replaces it and is kept selectable
        beside it for one release.
      </p>
      {#if settings.captureBackendError}
        <p class="setting-hint">{settings.captureBackendError}</p>
      {:else}
        <p class="setting-hint">{APPLIES_WHEN}</p>
        {#each notes as note (note)}
          <p class="setting-hint">{note}</p>
        {/each}
        {#if status}
          <p class="setting-hint">In use now: <span class="mono">{status.active}</span></p>
        {/if}
      {/if}
    {/snippet}
    <div class="segmented" role="radiogroup" aria-label="Capture backend">
      {#each status?.options ?? [] as option (option.backend)}
        <button
          type="button"
          role="radio"
          aria-checked={status?.configured === option.backend}
          disabled={settings.captureBackendBusy || option.unavailable !== null}
          title={option.unavailable ?? undefined}
          onclick={() => void saveCaptureBackend(option.backend)}
          >{BACKEND_LABELS[option.backend]}</button
        >
      {/each}
    </div>
  </SettingRow>

  {#if refusal}
    <p class="callout callout-warn">{refusal}</p>
  {/if}
</section>
