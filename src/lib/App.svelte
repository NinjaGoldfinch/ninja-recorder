<!--
  The Svelte root - WS4 task 4.1, filling up from WS4.3.

  It renders the views that have migrated and nothing else. `index.html` still
  holds the review section, and `main.ts` still wires it; the
  two frontends run side by side for the length of WS4, and each task moves one
  view across and deletes its vanilla counterpart in the same commit.

  **Each migrated view registers itself with the router.** `main.ts` registers
  the sections it still owns by `el("#id")`; a Svelte view has no id to look
  up, so it hands its own node over on mount. Either way `showView` stays the
  one thing that decides which section is showing, which is the invariant it
  was written for.

  Views still to come: `Review` and `Timeline` in WS4.5. WS4.6 deletes what
  is left of the markup.

  It imports no stylesheet. `styles/tokens.css` is pulled in by `styles.css`,
  which `index.html` loads as a `<link>` before first paint; a token block
  arriving later over a JS import is the theme flash the boot script in that
  file exists to prevent. WS4.6 re-homes the import when it deletes the
  stylesheet.

  **No `{@html}` on any recording-derived string** anywhere below this point.
  `db::reconcile` imports whatever video file the user drops into the folder,
  so a filename is untrusted input. Default interpolation is what replaces v1's
  `escapeHtml` and `escapeAttr`.
-->

<script lang="ts">
import { registerView } from "../router";
import Library from "./components/library/Library.svelte";
import Settings from "./components/settings/Settings.svelte";

let libraryNode: HTMLElement;
let settingsNode: HTMLElement;

// On mount, not in `main.ts`: these nodes do not exist until this renders,
// which is *after* `initRouting` has read the URL fragment. That ordering is
// why `registerView` sets `hidden` from the current view rather than trusting
// a node's default: a window opened at `#settings` would otherwise show the
// settings section and the library at once, because neither node was there
// for the `showView` that hid everything else.
$effect(() => {
  registerView("library", libraryNode);
  registerView("settings", settingsNode);
});
</script>

<section bind:this={libraryNode} id="library-view">
  <Library />
</section>

<section bind:this={settingsNode} id="settings-view" class="view" hidden>
  <Settings />
</section>
