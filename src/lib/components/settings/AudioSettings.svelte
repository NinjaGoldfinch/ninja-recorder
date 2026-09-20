<!--
  What gets captured, and on which tracks.

  The preview mirrors `AudioPreset::layout()` in Rust and is **for the preview
  only**: the backend decides what actually gets recorded.
-->

<script lang="ts">
import type { AudioPresetKey } from "../../../types";
import { micOptions, trackPreview } from "../../settings/audio";
import { saveAudioPreset, settings } from "../../stores/settings.svelte";
import SettingRow from "./SettingRow.svelte";

const PRESETS: { value: AudioPresetKey; label: string }[] = [
  { value: "game", label: "Game" },
  { value: "game_mic", label: "Game + mic" },
  { value: "game_mic_discord", label: "Game + mic + Discord" },
  { value: "desktop", label: "Desktop" },
];

const options = $derived(micOptions(settings.micDevices));
</script>

<section class="settings-group">
  <h3>Audio</h3>

  <SettingRow
    label="What to capture"
    hint="Your microphone is only ever recorded on a preset that names it."
  >
    <div class="segmented" role="radiogroup" aria-label="Audio capture">
      {#each PRESETS as preset (preset.value)}
        <button
          type="button"
          role="radio"
          aria-checked={settings.audioPreset === preset.value}
          onclick={() => void saveAudioPreset(preset.value)}>{preset.label}</button
        >
      {/each}
    </div>
  </SettingRow>

  <SettingRow
    label="Microphone"
    hint={"“Windows default” follows your default communications device, which is what the recorder asks for."}
  >
    <!--
      A device name comes from the driver, so it is untrusted text in the same
      way a filename is. `settings.ts` built these options as HTML with
      `escapeAttr` on the id and `escapeHtml` on the label; an `{#each}` needs
      neither.
    -->
    <select
      aria-label="Microphone"
      disabled={!settings.micEnabled}
      value={settings.micDeviceId}
      onchange={(e) =>
        void saveAudioPreset(settings.audioPreset, (e.currentTarget as HTMLSelectElement).value)}
    >
      {#each options as option (option.id)}
        <option value={option.id}>{option.label}</option>
      {/each}
    </select>
  </SettingRow>

  <div class="setting-block">
    <span class="setting-label">Tracks</span>
    <p class="setting-hint">
      Each recording gets one combined track plus an isolated track per source,
      so you can mute or extract any of them later.
    </p>
    <p class="callout">{trackPreview(settings.audioPreset)}</p>
  </div>
</section>
