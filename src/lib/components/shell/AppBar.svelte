<!--
  The header: who we are, what the client and the game are doing, and the two
  buttons that are not views.
-->

<script lang="ts">
import { call, hasDevCommands } from "../../../bridge";
import { showView } from "../../../router";
import { about } from "../../stores/about.svelte";
import { update } from "../../stores/update.svelte";

let devAvailable = $state(false);

// **Detected rather than configured.** The portal and its `dev_*` commands
// are compiled out unless the `devtools` Cargo feature is on, so the button
// asks the backend whether they are registered. A rejection is the expected
// outcome in a shipped build, which is why `hasDevCommands` answers `false`
// rather than throwing, and why the frontend carries no second flag that
// could drift from the Rust side.
void hasDevCommands().then((available: boolean) => {
  devAvailable = available;
});

function openDevPortal() {
  void call("dev_open_portal").catch((err) => {
    console.warn("dev portal unavailable:", err);
    devAvailable = false;
  });
}
</script>

<header class="app-bar">
  <div class="app-bar-inner">
    <div class="brand">
      <span class="brand-mark" aria-hidden="true">
        <svg viewBox="0 0 24 24" width="22" height="22" fill="none">
          <circle cx="12" cy="12" r="9" stroke="currentColor" stroke-width="1.6" />
          <circle cx="12" cy="12" r="3.4" fill="currentColor" />
        </svg>
      </span>
      <div class="brand-text">
        <span class="brand-name">ninja&#8209;recorder</span>
        <span class="brand-sub">VOD library</span>
      </div>
    </div>

    <div class="status-strip" role="status" aria-live="polite">
      <span class="status-pill" data-state={about.lcuPill.state}>
        <span class="status-dot" aria-hidden="true"></span>
        <span>{about.lcuPill.copy}</span>
      </span>
      <span class="status-pill" data-state={about.gamePill.state}>
        <span class="status-dot" aria-hidden="true"></span>
        <span>{about.gamePill.copy}</span>
      </span>
    </div>

    <button
      type="button"
      class="icon-btn ghost has-badge"
      aria-label="Settings"
      title="Settings"
      onclick={() => showView("settings")}
    >
      <!--
        **The entire announcement that an update exists.** Deliberately not a
        toast or a notification: CI releases every commit on main, so this
        would fire most days (DEVELOPMENT.md §14).
      -->
      {#if update.offering}
        <span class="badge">
          <span class="visually-hidden">An update is available</span>
        </span>
      {/if}
      <svg
        viewBox="0 0 24 24"
        width="18"
        height="18"
        fill="none"
        stroke="currentColor"
        stroke-width="1.7"
        stroke-linecap="round"
      >
        <circle cx="12" cy="12" r="3.2" />
        <path
          d="M19.4 15a1.7 1.7 0 0 0 .34 1.88l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.7 1.7 0 0 0-1.88-.34 1.7 1.7 0 0 0-1.03 1.56V21a2 2 0 1 1-4 0v-.09A1.7 1.7 0 0 0 8.9 19.3a1.7 1.7 0 0 0-1.88.34l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.7 1.7 0 0 0 .34-1.88 1.7 1.7 0 0 0-1.56-1.03H3a2 2 0 1 1 0-4h.09A1.7 1.7 0 0 0 4.7 8.9a1.7 1.7 0 0 0-.34-1.88l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.7 1.7 0 0 0 1.88.34H9.1a1.7 1.7 0 0 0 1.03-1.56V3a2 2 0 1 1 4 0v.09a1.7 1.7 0 0 0 1.03 1.56 1.7 1.7 0 0 0 1.88-.34l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.7 1.7 0 0 0-.34 1.88v.01a1.7 1.7 0 0 0 1.56 1.03H21a2 2 0 1 1 0 4h-.09a1.7 1.7 0 0 0-1.56 1.03z"
        />
      </svg>
    </button>

    {#if devAvailable}
      <button
        type="button"
        class="icon-btn ghost"
        aria-label="Dev portal"
        title="Dev portal"
        onclick={openDevPortal}
      >
        <svg
          viewBox="0 0 24 24"
          width="18"
          height="18"
          fill="none"
          stroke="currentColor"
          stroke-width="1.7"
          stroke-linecap="round"
          stroke-linejoin="round"
        >
          <path d="M9 3h6" />
          <path d="M10 3v6.2L4.9 18a2 2 0 0 0 1.7 3h10.8a2 2 0 0 0 1.7-3L14 9.2V3" />
          <path d="M7.6 14h8.8" />
        </svg>
      </button>
    {/if}
  </div>
</header>
