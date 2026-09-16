/**
 * What the window says when the recorder is not there. WS3 task 3.8.
 *
 * The daemon can go away for entirely ordinary reasons: the updater replaces
 * it, someone quits it from the tray, it crashes. The UI is disposable but the
 * *recording* is not, so the window has to be able to say "the recorder is not
 * running" rather than showing a library that quietly stops answering.
 *
 * ## A strip, not a toast
 *
 * A toast announces something that happened and then goes away. This is a
 * state: it is true until it stops being true, and it must clear itself when
 * the connection comes back rather than on a timer. Anything that disappeared
 * on its own while the daemon was still missing would be worse than saying
 * nothing.
 *
 * ## Three states, and one of them is terminal
 *
 * `reconnecting` is the ordinary one and resolves itself: `ui::client` retries
 * with a bounded backoff and `daemon::spawn` starts a daemon if none answers.
 * `skewed` does not resolve — a UI and a daemon from different builds cannot
 * be made to agree by waiting — so it says restart and offers nothing else.
 * `connected` hides the strip.
 *
 * WS4 replaces this with a store the snapshot and event stream feed. Until
 * then it is a listener and an element, which is what the rest of this
 * frontend is.
 */

import { el } from "./dom";
import { IN_TAURI } from "./lib/transport/invoke";
import { type DaemonHealth, daemonHealth, subscribe } from "./lib/transport/pipe";

let strip: HTMLElement | null = null;

export function initDaemonStatus() {
  strip = el("#daemon-strip");

  // Outside Tauri there is no daemon and no pipe, and the mock transport
  // answers everything. Showing "the recorder is not running" against the
  // fixtures would be true and useless.
  if (!IN_TAURI) return;

  // Asked once as well as subscribed, because a window that opens while the
  // daemon is already down would otherwise wait for the next change before
  // saying anything, and the next change may be minutes away.
  void daemonHealth()
    .then(render)
    .catch(() => {});
  subscribe({ onHealth: render });
}

function render(health: DaemonHealth) {
  if (!strip) return;

  switch (health.state) {
    case "connected":
      strip.hidden = true;
      return;
    case "reconnecting":
      strip.textContent =
        "The recorder is not running. Reconnecting… recordings in progress are unaffected.";
      strip.dataset.kind = "warn";
      strip.hidden = false;
      return;
    case "skewed":
      // Deliberately not offering a button. The two builds cannot agree, and
      // the one thing the UI must not do is tell the daemon to quit: it may be
      // recording.
      strip.textContent =
        `This window is from a different build than the recorder ` +
        `(protocol ${health.ours} against ${health.theirs}). Restart the app.`;
      strip.dataset.kind = "error";
      strip.hidden = false;
      return;
  }
}
