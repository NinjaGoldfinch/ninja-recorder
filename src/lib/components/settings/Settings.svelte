<!--
  The settings view - WS4 task 4.4.

  Replaces `settings.ts` and `update.ts`, which are both deleted, and the
  `#settings-view` markup with them.
-->

<script lang="ts">
import { whenDaemonReachable } from "../../../daemon";
import { showView } from "../../../router";
import {
  loadAudioSettings,
  loadAutostart,
  loadRecordingsDir,
  loadRetentionPolicy,
} from "../../stores/settings.svelte";
import About from "./About.svelte";
import Appearance from "./Appearance.svelte";
import AudioSettings from "./AudioSettings.svelte";
import BackgroundTray from "./BackgroundTray.svelte";
import Notifications from "./Notifications.svelte";
import Storage from "./Storage.svelte";

/**
 * **All four are RPCs, so all four wait for the daemon.**
 *
 * They used to run on load and lost the same race the library did: the
 * window paints before the handshake completes, `Client::call` answers
 * `Disconnected` by design, and each of these reported it as its own
 * failure. The visible one was the start-on-login row reading "Couldn't read
 * this setting: not connected to the recorder" on a perfectly healthy
 * window.
 *
 * Re-running on reconnect is right for a reason beyond symmetry:
 * `get_autostart` reads the registry rather than a cached pref, so what it
 * returns can change while the window is open.
 *
 * In the component rather than at startup because the component is the thing
 * that needs the answers, and mounting it is what makes the view exist.
 */
$effect(() => {
  whenDaemonReachable(() => {
    void loadAutostart();
    void loadRetentionPolicy();
    void loadRecordingsDir();
    void loadAudioSettings();
  });
});
</script>

<div class="view-header">
  <button type="button" class="back-btn" onclick={() => showView("library")}>
    &larr; Back
  </button>
  <h2>Settings</h2>
</div>

<Appearance />
<BackgroundTray />
<Notifications />
<AudioSettings />
<Storage />
<About />
