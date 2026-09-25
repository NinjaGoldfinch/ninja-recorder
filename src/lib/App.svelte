<!--
  The Svelte root - WS4 task 4.1, filling up from WS4.3.

  It renders every view. `index.html` is down to the app bar, the quit dialog
  and the anti-flash boot script, none of which is a view; WS4.6 takes those.

  **Each migrated view registers itself with the router.** `main.ts` registers
  the sections it still owns by `el("#id")`; a Svelte view has no id to look
  up, so it hands its own node over on mount. Either way `showView` stays the
  one thing that decides which section is showing, which is the invariant it
  was written for.

  Every view lives here now. WS4.6 deletes what is left of `index.html`: the
  app bar, the quit dialog and the boot script.

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
import Objectives from "./components/objectives/Objectives.svelte";
import Review from "./components/review/Review.svelte";
import ReviewForm from "./components/reviewform/ReviewForm.svelte";
import Settings from "./components/settings/Settings.svelte";
import AppBar from "./components/shell/AppBar.svelte";
import DaemonStrip from "./components/shell/DaemonStrip.svelte";
import QuitDialog from "./components/shell/QuitDialog.svelte";
import Toast from "./components/shell/Toast.svelte";
import { setQuitAsker } from "./stores/quit.svelte";

let libraryNode: HTMLElement;
let reviewNode: HTMLElement;
let settingsNode: HTMLElement;
let gameNode: HTMLElement;
let objectivesNode: HTMLElement;
let quitDialog: ReturnType<typeof QuitDialog> | undefined;

// On mount, not in `main.ts`: these nodes do not exist until this renders,
// which is *after* `initRouting` has read the URL fragment. That ordering is
// why `registerView` sets `hidden` from the current view rather than trusting
// a node's default: a window opened at `#settings` would otherwise show the
// settings section and the library at once, because neither node was there
// for the `showView` that hid everything else.
$effect(() => {
  registerView("library", libraryNode);
  registerView("review", reviewNode);
  registerView("settings", settingsNode);
  registerView("game", gameNode);
  registerView("objectives", objectivesNode);
});

// `quit.ts` owns the flow and this owns the dialog, so the flow is given a
// way to ask rather than a way to find an element.
$effect(() => {
  const dialog = quitDialog;
  setQuitAsker(dialog ? () => dialog.ask() : null);
  return () => setQuitAsker(null);
});
</script>

<AppBar />

<DaemonStrip />

<main class="container">
  <section bind:this={libraryNode} id="library-view">
    <Library />
  </section>

  <section bind:this={reviewNode} id="review-view" class="review-view" hidden>
    <Review />
  </section>

  <section bind:this={settingsNode} id="settings-view" class="view" hidden>
    <Settings />
  </section>

  <section bind:this={gameNode} id="game-view" class="view" hidden>
    <ReviewForm />
  </section>

  <section bind:this={objectivesNode} id="objectives-view" class="view" hidden>
    <Objectives />
  </section>
</main>

<QuitDialog bind:this={quitDialog} />
<Toast />
