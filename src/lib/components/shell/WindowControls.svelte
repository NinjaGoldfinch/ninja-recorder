<!--
  Minimise, maximise and close, for a window with no native title bar.

  Drawn to sit where Windows puts its own and to behave like them: full height
  of the bar, close turning red on hover. **Close is `close()`, not `destroy()`
  or `hide()`**: it raises `CloseRequested`, which is where the close-to-tray
  setting and the quit prompt already live, so this button and the native one
  it replaces cannot disagree about what closing means.

  Outside the Tauri webview (the browser dev server) there is no window to
  drive, and the buttons are left out rather than drawn doing nothing.
-->

<script lang="ts">
import { getCurrentWindow } from "@tauri-apps/api/window";
import { IN_TAURI } from "../../transport/invoke";

const win = IN_TAURI ? getCurrentWindow() : null;
let maximized = $state(false);

// Resizing is the one signal that covers every way the window can change
// state: these buttons, a double-click on the bar, Win+Up, and snapping.
$effect(() => {
  if (!win) return;
  const sync = () => void win.isMaximized().then((m) => (maximized = m));
  sync();
  const unlisten = win.onResized(sync);
  return () => void unlisten.then((stop) => stop());
});
</script>

{#if win}
  <div class="window-controls">
    <button
      type="button"
      class="wc-btn"
      aria-label="Minimise"
      title="Minimise"
      onclick={() => void win.minimize()}
    >
      <svg viewBox="0 0 10 10" width="10" height="10" aria-hidden="true">
        <path d="M0 5.5h10" stroke="currentColor" stroke-width="1" />
      </svg>
    </button>
    <button
      type="button"
      class="wc-btn"
      aria-label={maximized ? "Restore" : "Maximise"}
      title={maximized ? "Restore" : "Maximise"}
      onclick={() => void win.toggleMaximize()}
    >
      {#if maximized}
        <svg viewBox="0 0 10 10" width="10" height="10" aria-hidden="true" fill="none">
          <rect x="0.5" y="2.5" width="7" height="7" stroke="currentColor" stroke-width="1" />
          <path d="M2.5 2.5v-2h7v7h-2" stroke="currentColor" stroke-width="1" />
        </svg>
      {:else}
        <svg viewBox="0 0 10 10" width="10" height="10" aria-hidden="true" fill="none">
          <rect x="0.5" y="0.5" width="9" height="9" stroke="currentColor" stroke-width="1" />
        </svg>
      {/if}
    </button>
    <button
      type="button"
      class="wc-btn wc-close"
      aria-label="Close"
      title="Close"
      onclick={() => void win.close()}
    >
      <svg viewBox="0 0 10 10" width="10" height="10" aria-hidden="true">
        <path d="M0 0l10 10M10 0 0 10" stroke="currentColor" stroke-width="1" />
      </svg>
    </button>
  </div>
{/if}
