/**
 * What "Quit" means, now that the recorder is a different process. WS3, #136.
 *
 * The close button's `quit` option used to end the window and nothing else.
 * The daemon kept recording, kept its tray icon, and the app was visibly still
 * running: the setting said one thing and did another, which is worse than not
 * offering it.
 *
 * ## Two calls, in this order
 *
 * `quit_recorder` goes over the pipe and stops the recorder, finalizing
 * whatever is in flight on its way out. `exit_ui` stays in this process and
 * ends the window. Both, in that order, because they are two processes and the
 * answer for each is different: the window always goes, the recorder only goes
 * if the person said so.
 *
 * ## The question belongs in this window
 *
 * The daemon has a confirmation of its own for tray Quit, a `MessageBoxW` with
 * no owner window. That is right for a tray click and wrong for this one: a
 * modal with no owner can appear *behind* the window the person just clicked,
 * and they would be looking at a window that had stopped responding to its own
 * close button. So the daemon refuses instead, answers `recordingInFlight`,
 * and the dialog below is the one that asks.
 */

import { call } from "./bridge";
import { el } from "./dom";
import { toast } from "./toast";

/** What `quit_recorder` answered. Mirrors `core::QuitOutcome`. */
type QuitOutcome = { outcome: "shuttingDown" } | { outcome: "recordingInFlight" };

let dialog: HTMLDialogElement | null = null;

export function initQuit() {
  dialog = el<HTMLDialogElement>("#quit-dialog");
}

/**
 * Asks, and answers whether to go ahead.
 *
 * `<dialog>` rather than `window.confirm`, which WebView2 renders as a bare
 * system box with the executable's path in the title, and which blocks the
 * whole webview while it is up.
 */
function askAboutTheRecording(): Promise<boolean> {
  return new Promise((resolve) => {
    if (!dialog) {
      // No dialog means no way to ask, and quitting anyway would end a
      // recording nobody agreed to lose.
      resolve(false);
      return;
    }
    const done = (quit: boolean) => {
      dialog?.removeEventListener("close", onClose);
      resolve(quit);
    };
    // Covers Escape and any other dismissal, both of which mean "no".
    const onClose = () => done(dialog?.returnValue === "quit");
    dialog.addEventListener("close", onClose, { once: true });
    dialog.returnValue = "";
    dialog.showModal();
  });
}

/**
 * The whole flow, from the close button to a process that is gone.
 *
 * Exported for `main.ts` to hang off the `quit-requested` event that `lib.rs`
 * raises when the close action is `quit`.
 */
export async function quitEverything() {
  try {
    let answer = await call<QuitOutcome>("quit_recorder", { force: false });

    if (answer.outcome === "recordingInFlight") {
      if (!(await askAboutTheRecording())) return;
      // Asked again with the person's answer rather than trusting the first
      // reply: a game can end between the question and the click, and the
      // daemon deciding twice is cheaper than this side deciding once.
      answer = await call<QuitOutcome>("quit_recorder", { force: true });
    }

    if (answer.outcome !== "shuttingDown") {
      toast("The recorder would not stop, so nothing was quit.", "error");
      return;
    }

    await call("exit_ui", {});
  } catch (err) {
    // Reported rather than swallowed: the window is still here, the close
    // button appears to have done nothing, and the reason is the only thing
    // that makes that intelligible.
    toast(`Could not quit: ${err}`, "error");
  }
}
