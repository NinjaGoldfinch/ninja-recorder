<!--
  Settings → Advanced: which capture backend records. WS1.7, #11.

  Every choice the daemon knows about is listed, and one this build cannot
  construct is shown disabled with the daemon's reason rather than left out:
  the own backend needs Windows build 19041 or newer, and a missing option
  would say nothing about why. The reasons and the live backend's name come
  from the daemon, and are interpolated, never rendered as markup.

  Shown in every build since #243 made the own backend the default: libobs is
  the fallback a user can pick without a reinstall, for one release.
-->

<script lang="ts">
import {
  APPLIES_WHEN,
  automaticNote,
  BACKEND_EXPLAINED,
  BACKEND_LABELS,
  refusalNote,
  softwareNote,
  unavailableNotes,
} from "../../settings/capture";
import { saveCaptureBackend, settings } from "../../stores/settings.svelte";
import SettingRow from "./SettingRow.svelte";

const status = $derived(settings.captureBackend);
const notes = $derived(status ? unavailableNotes(status) : []);
const refusal = $derived(status ? refusalNote(status) : null);
const software = $derived(status ? softwareNote(status) : null);
const automatic = $derived(status ? automaticNote(status) : null);
</script>

<section class="settings-group">
  <h3>Advanced</h3>

  <SettingRow label="Capture backend">
    {#snippet copy()}
      <p class="setting-hint">Which engine records your games.</p>
      {#each status?.options ?? [] as option (option.backend)}
        <p class="setting-hint">
          <strong>{BACKEND_LABELS[option.backend]}</strong>: {BACKEND_EXPLAINED[option.backend]}
        </p>
      {/each}
      {#if settings.captureBackendError}
        <p class="setting-hint">{settings.captureBackendError}</p>
      {:else}
        {#if automatic}
          <p class="setting-hint">{automatic}</p>
        {/if}
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

  {#if software}
    <p class="callout callout-warn">{software}</p>
  {/if}

  {#if refusal}
    <p class="callout callout-warn">{refusal}</p>
  {/if}
</section>
