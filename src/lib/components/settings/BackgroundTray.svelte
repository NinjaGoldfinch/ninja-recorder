<!-- Start on login, and what the close button does. -->

<script lang="ts">
import type { CloseActionPref } from "../../../prefs";
import { setAutostart, setPref, settings } from "../../stores/settings.svelte";
import SettingRow from "./SettingRow.svelte";

const DEFAULT_HINT =
  "Launch ninja-recorder into the tray when you sign in, with no window, " +
  "so a game is recorded even if you never opened it.";

// Three states, and they are not the same thing: a read that failed leaves
// an error here and a later success has to clear it, which is why the hint
// is derived rather than written into and restored.
const hint = $derived(
  settings.autostartError ??
    (settings.autostart && !settings.autostart.supported
      ? "Not available in this build."
      : DEFAULT_HINT),
);

const CLOSE_ACTIONS: { value: CloseActionPref; label: string }[] = [
  { value: "close-window", label: "Close the window (keeps recording)" },
  { value: "hide", label: "Hide the window (keeps recording)" },
  { value: "quit", label: "Quit ninja-recorder (stops recording)" },
];
</script>

<section class="settings-group">
  <h3>Background &amp; tray</h3>

  <SettingRow label="Start on login" {hint}>
    <input
      type="checkbox"
      aria-label="Start on login"
      checked={settings.autostart?.enabled ?? false}
      disabled={settings.autostartBusy ||
        settings.autostartError !== null ||
        !(settings.autostart?.supported ?? false)}
      onchange={(e) => void setAutostart((e.currentTarget as HTMLInputElement).checked)}
    />
  </SettingRow>

  <SettingRow label="Close button">
    {#snippet copy()}
      <p class="setting-hint">
        The recorder runs as its own background process, so closing the window
        and quitting are not the same thing.
      </p>
      <ul class="setting-hint setting-choices">
        <li>
          <strong>Close the window</strong> frees the memory the window was using.
          Recording carries on, and the tray icon reopens it.
        </li>
        <li>
          <strong>Hide the window</strong> keeps it loaded so reopening is instant,
          at the cost of its memory the whole time. Recording carries on.
        </li>
        <li>
          <strong>Quit ninja-recorder</strong> stops the recorder too.
          <strong>Nothing is recorded in the background afterwards</strong>, until
          you open the app again. If a game is being recorded it asks first, and
          finishes saving that recording before exiting.
        </li>
      </ul>
    {/snippet}
    <!--
      Fire-and-forget, unlike the audio preset: this only decides what the
      close button does, and Rust re-reads it from SQLite on every close, so a
      slow write cannot desync anything.
    -->
    <select
      aria-label="Close button"
      value={settings.prefs.closeAction}
      onchange={(e) =>
        setPref("closeAction", (e.currentTarget as HTMLSelectElement).value as CloseActionPref)}
    >
      {#each CLOSE_ACTIONS as action (action.value)}
        <option value={action.value}>{action.label}</option>
      {/each}
    </select>
  </SettingRow>
</section>
