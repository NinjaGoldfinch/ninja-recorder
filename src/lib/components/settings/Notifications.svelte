<!--
  The notification switches.

  Rust reads these straight out of SQLite when it is about to notify, so there
  is nothing to keep in sync on this side beyond the checkbox state.
-->

<script lang="ts">
import type { NotifyPrefKey } from "../../../prefs";
import { resetNotices, setPref, settings } from "../../stores/settings.svelte";
import SettingRow from "./SettingRow.svelte";

const EVENTS: { key: NotifyPrefKey; label: string; hint: string; aria: string }[] = [
  {
    key: "notifyRecordingStarted",
    label: "Recording started",
    hint: "Off by default — you are about to be in a game.",
    aria: "Notify when recording starts",
  },
  {
    key: "notifyRecordingFinished",
    label: "Recording finished",
    hint: "Tells you the VOD was saved, and how many markers it has.",
    aria: "Notify when a recording is saved",
  },
  {
    key: "notifyRecordingFailed",
    label: "Recording problems",
    hint:
      "A recording that failed to start or finish, or a disk too full to " +
      "record. The only way to find out mid-game.",
    aria: "Notify about recording problems",
  },
];

// **The master switch gates the rest in Rust**, so the form says so rather
// than leaving three checkboxes that look live and do nothing.
const enabled = $derived(settings.prefs.notifications === "on");

const toggle = (key: NotifyPrefKey | "notifications", on: boolean) =>
  setPref(key, on ? "on" : "off");
</script>

<section class="settings-group">
  <h3>Notifications</h3>

  <SettingRow
    label="Notifications"
    hint="Desktop notifications while ninja-recorder is in the tray. Turning this off silences all of them."
  >
    <input
      type="checkbox"
      aria-label="Notifications"
      checked={enabled}
      onchange={(e) => toggle("notifications", (e.currentTarget as HTMLInputElement).checked)}
    />
  </SettingRow>

  {#each EVENTS as event (event.key)}
    <SettingRow label={event.label} hint={event.hint}>
      <input
        type="checkbox"
        aria-label={event.aria}
        checked={settings.prefs[event.key] === "on"}
        disabled={!enabled}
        onchange={(e) => toggle(event.key, (e.currentTarget as HTMLInputElement).checked)}
      />
    </SettingRow>
  {/each}

  <SettingRow
    label="One-time notices"
    hint={"Show the “still running in the tray” notice again next time you close the window."}
  >
    <button type="button" onclick={resetNotices}>Reset</button>
  </SettingRow>
</section>
