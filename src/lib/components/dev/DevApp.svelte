<!--
  The dev portal - #72.
 
  Replaces `src/dev/main.ts`: the hash router, the shared 1 Hz poll, the top
  bar and the panel mount.

  **`freshMain()` is gone, and it is the clearest thing this migration buys.**
  Panels bound delegated handlers to the element they were handed and had no
  way to unbind them, so mounting onto the same element stacked one handler per
  mount. `refresh()` remounts on every `r`, every `library-changed` and every
  panel that calls it, so two live handlers turned one click into two toggles:
  the Log panel's tag chips lit up and reverted inside the same frame. Worse,
  the stale handlers belonged to *other* panels, and hooks like `[data-reload]`
  were not unique across them. The workaround was to shallow-clone `#dev-main`
  on every navigation so each panel got a listener-free element.

  Svelte removes the handlers with the component that owns them, so there is
  nothing to clone and nothing to explain.
-->

<script lang="ts">
import { listen } from "@tauri-apps/api/event";
import { setDevContext } from "../../dev/context";
import { routeFor } from "../../dev/panels";
import type { ConfirmOptions } from "../../dev/ui-types";
import { dev, loadEnv, setLive, stopPolling } from "../../stores/dev.svelte";
import { devToast } from "../../stores/devToast.svelte";
import ConfirmDialog from "./ConfirmDialog.svelte";
import DevToasts from "./DevToasts.svelte";
import Output from "./Output.svelte";
import Commands from "./panels/Commands.svelte";
import Database from "./panels/Database.svelte";
import Diagnostics from "./panels/Diagnostics.svelte";
import Fixtures from "./panels/Fixtures.svelte";
import Library from "./panels/Library.svelte";
import Log from "./panels/Log.svelte";
import Overview from "./panels/Overview.svelte";
import Recorder from "./panels/Recorder.svelte";
import Retention from "./panels/Retention.svelte";
import Seed from "./panels/Seed.svelte";
import Simulate from "./panels/Simulate.svelte";
import SideBar from "./SideBar.svelte";
import TopBar from "./TopBar.svelte";

/**
 * Every panel but Library, which is rendered on its own below.
 *
 * A component that declares no props types as `Record<string, never>`, so one
 * map cannot both hold those ten and accept `payload` on the eleventh. The
 * deep link belongs to exactly one panel, and saying so here costs less than
 * declaring a prop in ten files that ignore it.
 */
const PANEL_COMPONENTS: Record<string, typeof Overview> = {
  overview: Overview,
  recorder: Recorder,
  simulate: Simulate,
  database: Database,
  seed: Seed,
  retention: Retention,
  fixtures: Fixtures,
  commands: Commands,
  diagnostics: Diagnostics,
  log: Log,
};

let hash = $state(location.hash);
const route = $derived(routeFor(hash));

let confirmDialog = $state<ReturnType<typeof ConfirmDialog>>();

/** Set by `navigate`, consumed by the panel it was meant for. */
let pendingHandoff: string | null = null;

/**
 * Bumped to force a remount of the current panel.
 *
 * The one thing `refresh()` was for that state alone does not cover: `r`
 * re-runs a panel's own load from scratch, which is what you want after
 * changing the database from somewhere else.
 */
let generation = $state(0);

setDevContext({
  get env() {
    return dev.env;
  },
  navigate(id, handoff) {
    // Held here rather than put in the hash: see `DevContext.navigate`.
    pendingHandoff = handoff ?? null;
    location.hash = `#/${id}`;
  },
  takeHandoff() {
    const value = pendingHandoff;
    pendingHandoff = null;
    return value;
  },
  confirm(options: ConfirmOptions) {
    return confirmDialog?.ask(options) ?? Promise.resolve(false);
  },
});

$effect(() => {
  document.title = `${route.panel.title} — ninja-recorder dev portal`;
});

$effect(() => {
  void loadEnv();
  setLive(true);

  const onHash = () => {
    hash = location.hash;
  };
  window.addEventListener("hashchange", onHash);

  // The portal is usually driving these, but the supervisor emits it too: a
  // real game finishing while the portal is open should show up here as well
  // as in the main window.
  const unlisten = listen("library-changed", () => {
    if (["database", "seed", "retention"].includes(route.panel.id)) generation += 1;
  }).catch((err) => {
    console.warn("library-changed listener unavailable:", err);
    return () => {};
  });

  const onKey = (e: KeyboardEvent) => {
    if (e.key !== "r" || e.metaKey || e.ctrlKey) return;
    const el = document.activeElement;
    if (
      el instanceof HTMLInputElement ||
      el instanceof HTMLTextAreaElement ||
      el instanceof HTMLSelectElement
    ) {
      return;
    }
    generation += 1;
    devToast("Refreshed");
  };
  document.addEventListener("keydown", onKey);

  return () => {
    window.removeEventListener("hashchange", onHash);
    document.removeEventListener("keydown", onKey);
    void unlisten.then((stop) => stop());
    stopPolling();
  };
});
</script>

<div class="dev-root">
  <TopBar />
  <SideBar activeId={route.panel.id} />

  <main class="dev-main" tabindex="-1">
    {#if dev.envError}
      <!--
        Reaching `dev.html` in a build without the `devtools` feature is the one
        failure worth spelling out. Every panel would otherwise show an opaque
        "command not found".
      -->
      <div class="warnbar warnbar-danger">
        <strong>Dev commands are not available in this build.</strong>
        Run the app with <code>npm run tauri:dev</code>, which passes
        <code>--features devtools</code>.
      </div>
      <Output value={dev.envError} isError />
    {:else}
      {#key `${route.panel.id}:${generation}`}
        {#if route.panel.id === "library"}
          <Library payload={route.payload} />
        {:else}
          {@const Panel = PANEL_COMPONENTS[route.panel.id]}
          <Panel />
        {/if}
      {/key}
    {/if}
  </main>

  <ConfirmDialog bind:this={confirmDialog} />
  <DevToasts />
</div>
