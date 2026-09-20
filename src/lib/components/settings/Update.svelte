<!--
  The update row in Settings → About.

  **Quiet about interrupting, not about telling.** CI publishes a release for
  every commit on `main`, so "something newer exists" is true most days, and a
  toast on each one is noise the user learns to dismiss without reading. The
  announcement is a dot; what this row says is as much as it has, because
  deciding whether to restart mid-session is the user's call and they cannot
  make it from a version number alone. See DEVELOPMENT.md §14.
-->

<script lang="ts">
import type { UpdateChannelPref } from "../../../prefs";
import { setPref, settings } from "../../stores/settings.svelte";
import {
  checkNow,
  install,
  installingText,
  recheckForChannel,
  update,
} from "../../stores/update.svelte";
import UpdateNotes from "./UpdateNotes.svelte";

const CHANNELS: { value: UpdateChannelPref; label: string }[] = [
  { value: "stable", label: "Stable — releases only" },
  { value: "alpha", label: "Alpha — every build" },
];

// A download in flight owns the row. `update.installing` is what stops a
// background check putting the buttons back under the user mid-install.
const text = $derived(update.installing ? installingText() : (update.row?.text ?? ""));

function chooseChannel(value: UpdateChannelPref) {
  setPref("updateChannel", value);
  // Check straight away rather than leaving the panel describing the channel
  // they just left. `savePref` is fire-and-forget, but Rust reads the pref
  // from SQLite when the check runs, so the write has to land first.
  void recheckForChannel();
}
</script>

<div>
  <dt>Update channel</dt>
  <dd>
    <select
      aria-describedby="update-channel-hint"
      value={settings.prefs.updateChannel}
      onchange={(e) =>
        chooseChannel((e.currentTarget as HTMLSelectElement).value as UpdateChannelPref)}
    >
      {#each CHANNELS as channel (channel.value)}
        <option value={channel.value}>{channel.label}</option>
      {/each}
    </select>
    <p id="update-channel-hint" class="setting-hint">
      Alpha publishes on every commit and is not tested before release.
      Switching back to Stable offers nothing until a release overtakes the
      alpha you are on.
    </p>
  </dd>
</div>

<div>
  <dt>Updates</dt>
  <dd>
    <span>{text}</span>
    <span class="about-update-actions">
      <button
        type="button"
        class="ghost small"
        disabled={update.checking || update.installing}
        onclick={() => void checkNow()}>Check now</button
      >
      {#if update.row?.offering}
        <button
          type="button"
          class="primary small"
          disabled={!update.row.installable || update.installing}
          onclick={() => void install()}>Install and restart</button
        >
      {/if}
    </span>
    <!--
      The changelog stays up while a download runs. It is what the user was
      reading to decide, and removing it the instant they act is the one moment
      it is least welcome.
    -->
    {#if update.row?.offering}
      <UpdateNotes notes={update.row.notes} />
    {/if}
  </dd>
</div>
