<!-- The recordings folder, the backfill, and the retention policy. -->

<script lang="ts">
import {
  openRecordingsFolder,
  previewRetention,
  runBackfill,
  saveRetentionPolicy,
  settings,
} from "../../stores/settings.svelte";
import SettingRow from "./SettingRow.svelte";

/**
 * The preview is debounced, not asked for on every keystroke.
 *
 * It is a round trip that walks the library, and the number field fires
 * `input` per digit: typing "120" would ask three questions and race their
 * answers into one callout.
 */
let previewTimer: ReturnType<typeof setTimeout> | undefined;
function schedulePreview() {
  clearTimeout(previewTimer);
  previewTimer = setTimeout(() => void previewRetention(), 350);
}
$effect(() => () => clearTimeout(previewTimer));

function submit(e: Event) {
  e.preventDefault();
  void saveRetentionPolicy();
}
</script>

<section class="settings-group">
  <h3>Storage</h3>

  <SettingRow label="Recordings folder">
    {#snippet copy()}
      <p class="setting-hint mono">{settings.recordingsDir ?? "—"}</p>
    {/snippet}
    <button type="button" onclick={() => void openRecordingsFolder()}>Open folder</button>
  </SettingRow>

  <SettingRow label="Fill in missing match data">
    {#snippet copy()}
      <p class="setting-hint">
        Recordings made before ninja-recorder could read match data &mdash; and
        anything imported by a rescan &mdash; show &ldquo;&mdash;&rdquo; for
        champion and result. This matches them against your League client&rsquo;s
        match history by when they were played. Needs the client running, and
        only reaches back as far as its history does.
      </p>
      <p class="setting-hint">
        It cannot bring back the timeline. Kills, objectives and the kill and CS
        curves were only ever seen by watching the game as it happened, so
        recordings made before that watching existed keep an empty timeline no
        matter how complete their row becomes.
      </p>
    {/snippet}
    <button type="button" disabled={settings.backfillBusy} onclick={() => void runBackfill()}>
      Fill in
    </button>
  </SettingRow>

  {#if settings.backfillReport}
    <p class="callout">{settings.backfillReport}</p>
  {/if}

  <div class="setting-block">
    <span class="setting-label">Retention policy</span>
    <p class="setting-hint">
      Pinned recordings are exempt from both limits. When a limit is hit, the
      oldest non&#8209;pinned recordings are removed first &mdash; checked on app
      start and after every recording finishes.
    </p>

    <form class="retention-form" onsubmit={submit}>
      <label class="retention-field">
        <input
          type="checkbox"
          bind:checked={settings.retention.sizeEnabled}
          onchange={schedulePreview}
        />
        Max total size (GB)
        <input
          type="number"
          min="1"
          step="1"
          disabled={!settings.retention.sizeEnabled}
          bind:value={settings.retention.sizeGb}
          oninput={schedulePreview}
        />
      </label>
      <label class="retention-field">
        <input
          type="checkbox"
          bind:checked={settings.retention.ageEnabled}
          onchange={schedulePreview}
        />
        Max age (days)
        <input
          type="number"
          min="1"
          step="1"
          disabled={!settings.retention.ageEnabled}
          bind:value={settings.retention.ageDays}
          oninput={schedulePreview}
        />
      </label>
      <button type="submit" class="primary">Save</button>
      <span class="status">{settings.retentionStatus}</span>
    </form>

    <!--
      **Said before it happens, not after.** This form is the one place in the
      app where a careless edit destroys footage.
    -->
    {#if settings.retentionPreview}
      <p class="callout callout-warn">{settings.retentionPreview}</p>
    {/if}
    {#if settings.retentionReport}
      <p class="callout">{settings.retentionReport}</p>
    {/if}
  </div>
</section>
