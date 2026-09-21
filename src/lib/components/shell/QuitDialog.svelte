<!--
  Whether to stop a recording in order to quit - WS3, #136.

  **The question belongs in this window.** The daemon has a confirmation of its
  own for tray Quit, a `MessageBoxW` with no owner window. That is right for a
  tray click and wrong for this one: a modal with no owner can appear *behind*
  the window the person just clicked, and they would be looking at a window
  that had stopped responding to its own close button. So the daemon refuses
  instead, answers `recordingInFlight`, and this is what asks.

  `<dialog>` rather than `window.confirm`, which WebView2 renders as a bare
  system box with the executable's path in the title, and which blocks the
  whole webview while it is up.
-->

<script lang="ts">
let dialog = $state<HTMLDialogElement>();

/** Set while a question is outstanding; resolved by the `close` event. */
let answer: ((quit: boolean) => void) | null = null;

/**
 * What the person chose, recorded on click.
 *
 * **Not read back off `returnValue` in the close handler**, which is the
 * obvious way to do this and the fragile one: the order in which a `<dialog>`
 * sets `returnValue` relative to firing `close` is not something to depend on,
 * and jsdom and the browser do not agree about it. Recording the choice where
 * the choice is made has no such question.
 *
 * Defaults to `false` every time the dialog opens, so any dismissal that is
 * not one of the two buttons means "no".
 */
let chose = false;

/**
 * Asks, and resolves with whether to go ahead.
 *
 * Exported so `quit.svelte.ts`'s flow can await it; `App.svelte` hands it over
 * with `setQuitAsker`.
 */
export function ask(): Promise<boolean> {
  if (!dialog) {
    // No dialog means no way to ask, and quitting anyway would end a
    // recording nobody agreed to lose.
    return Promise.resolve(false);
  }
  return new Promise<boolean>((resolve) => {
    answer = resolve;
    chose = false;
    dialog?.showModal();
  });
}

function choose(quit: boolean) {
  chose = quit;
  dialog?.close();
}

// Covers Escape and any other dismissal, both of which mean "no" because
// `chose` is still false.
function onClose() {
  const resolve = answer;
  answer = null;
  resolve?.(chose);
}
</script>

<dialog class="quit-dialog" bind:this={dialog} onclose={onClose}>
  <!--
    **The buttons are `type="button"` and close the dialog themselves**, rather
    than relying on `method="dialog"` to do it. Submitting a form closes the
    dialog as a *default action*, which races the click handler that records
    which button it was: the close event can arrive before `chose` is set, and
    "Quit anyway" then answers no. Closing explicitly puts the two in an order
    that does not depend on the environment.

    Escape still works and still means no, because it fires `close` without
    going through either button and `chose` is false by then.
  -->
  <form method="dialog">
    <h2>Quit while recording?</h2>
    <p>
      A game is being recorded right now. Quitting finishes and saves that
      recording first, which takes a few seconds, and then nothing is recorded
      in the background until you open ninja-recorder again.
    </p>
    <div class="quit-dialog-actions">
      <button type="button" class="ghost" onclick={() => choose(false)}>Keep recording</button>
      <button type="button" class="danger" onclick={() => choose(true)}>Quit anyway</button>
    </div>
  </form>
</dialog>
