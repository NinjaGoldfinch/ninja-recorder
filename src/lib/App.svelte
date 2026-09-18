<!--
  The Svelte root - WS4 task 4.1, filling up from WS4.3.

  It renders the views that have migrated and nothing else. `index.html` still
  holds the review and settings sections, and `main.ts` still wires them; the
  two frontends run side by side for the length of WS4, and each task moves one
  view across and deletes its vanilla counterpart in the same commit.

  **Each migrated view registers itself with the router.** `main.ts` registers
  the sections it still owns by `el("#id")`; a Svelte view has no id to look
  up, so it hands its own node over on mount. Either way `showView` stays the
  one thing that decides which section is showing, which is the invariant it
  was written for.

  Views still to come: `Settings` and `Update` in WS4.4, `Review` and
  `Timeline` in WS4.5. WS4.6 deletes what is left of the markup.

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

let libraryNode: HTMLElement;

// On mount, not in `main.ts`: the node does not exist until this renders.
// `initRouting` runs before this and reads the URL fragment, so a window
// opened at `#settings` has already switched by the time the library
// registers, and `showView` sets `hidden` on every registered node when it
// does. Registering late therefore cannot leave two views showing, because
// the registration itself does not change what is current.
$effect(() => {
  registerView("library", libraryNode);
});
</script>

<section bind:this={libraryNode} id="library-view">
  <Library />
</section>
