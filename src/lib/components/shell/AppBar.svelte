<!--
  The title bar. The window has no native one (`lib.rs::create_main_window`),
  so this is where it is dragged from, double-clicked to maximise, and closed.

  Left to right: who we are, the two top-level views as tabs, empty bar to
  drag by, the League client pill, the settings and dev-portal buttons, and the
  window controls.

  **The drag region is `deep`**, so a press anywhere in the bar drags the
  window unless it lands on something clickable: Tauri's drag script treats a
  button, a link or anything with a `tabindex` as a hole in the region, which
  is what keeps the tabs and the pill working without marking each one.
-->

<script lang="ts">
import { call, hasDevCommands } from "../../../bridge";
import { currentView, onViewChange, showView, type View } from "../../../router";
import { settings } from "../../stores/settings.svelte";
import { update } from "../../stores/update.svelte";
import ClientStatus from "./ClientStatus.svelte";
import WindowControls from "./WindowControls.svelte";

let devAvailable = $state(false);
let view = $state<View>(currentView());

// The review belongs to the library tab: it is opened from a library row, and
// the tab is the way back.
const tab = $derived(view === "review" ? "library" : view);

$effect(() => onViewChange((next) => (view = next)));

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

<header class="app-bar" data-tauri-drag-region="deep">
  <div class="app-bar-inner">
    <div class="brand">
      <span class="brand-mark" aria-hidden="true">
        <svg viewBox="0 0 24 24" width="15" height="15" fill="none">
          <circle cx="12" cy="12" r="9" stroke="currentColor" stroke-width="2" />
          <circle cx="12" cy="12" r="3.6" fill="currentColor" />
        </svg>
      </span>
      <span class="brand-name">ninja&#8209;recorder</span>
      <span class="brand-version selectable">v{settings.version}</span>
    </div>

    <span class="app-bar-sep" aria-hidden="true"></span>

    <nav class="app-tabs" aria-label="Views">
      <button
        type="button"
        class="app-tab"
        aria-current={tab === "library" ? "page" : undefined}
        onclick={() => showView("library")}
      >
        <svg
          viewBox="0 0 24 24"
          width="15"
          height="15"
          fill="none"
          stroke="currentColor"
          stroke-width="1.8"
          stroke-linecap="round"
          stroke-linejoin="round"
          aria-hidden="true"
        >
          <rect x="3" y="4" width="18" height="16" rx="2" />
          <path d="m10 9 5 3-5 3z" />
        </svg>
        Library
      </button>
      <button
        type="button"
        class="app-tab"
        aria-current={tab === "objectives" ? "page" : undefined}
        onclick={() => showView("objectives")}
      >
        <svg
          viewBox="0 0 24 24"
          width="15"
          height="15"
          fill="none"
          stroke="currentColor"
          stroke-width="1.8"
          stroke-linecap="round"
          stroke-linejoin="round"
          aria-hidden="true"
        >
          <path d="M9 6h11" />
          <path d="M9 12h11" />
          <path d="M9 18h11" />
          <path d="m3.5 6 1.5 1.5L7.5 5" />
          <path d="m3.5 12 1.5 1.5L7.5 11" />
          <path d="m3.5 18 1.5 1.5L7.5 17" />
        </svg>
        Objectives
      </button>
    </nav>

    <span class="app-bar-fill"></span>

    <ClientStatus />

    <span class="app-bar-sep" aria-hidden="true"></span>

    <button
      type="button"
      class="icon-btn ghost has-badge"
      aria-label="Settings"
      aria-current={tab === "settings" ? "page" : undefined}
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

    <WindowControls />
  </div>
</header>
