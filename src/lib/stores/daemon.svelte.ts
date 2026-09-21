/**
 * What the window says when the recorder is not there - WS3 task 3.8, moved
 * to a store by WS4.6.
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
 * `skewed` does not resolve, because a UI and a daemon from different builds
 * cannot be made to agree by waiting, so it says restart and offers nothing
 * else. `connected` hides the strip.
 *
 * ## It is also where "is the recorder reachable" is answered
 *
 * `whenDaemonReachable` exists because a cold start is a race the window
 * loses. `DaemonLink::connect` returns immediately and keeps connecting in the
 * background, so the first paint happens before the handshake, and on a first
 * launch after an install the UI is starting the daemon itself, which takes
 * seconds. Anything that fetched on load got `not connected to the recorder`
 * back and reported it as a failure, which is how a red error box came to
 * greet a perfectly healthy install.
 *
 * Waiting is the fix rather than swallowing the error. A call that is not made
 * cannot fail spuriously, and a call that fails while the connection is up is
 * a real failure that should still be said out loud.
 */

import { IN_TAURI } from "../transport/invoke";
import { type DaemonHealth, daemonHealth, subscribe } from "../transport/pipe";

/**
 * The last health reported, or `undefined` before the first answer.
 *
 * **The three-way distinction matters**: "not yet known" is not "not
 * connected". Treating it as connected would race the handshake, and treating
 * it as disconnected would be a claim nothing has made yet.
 */
let current = $state<DaemonHealth | undefined>(undefined);

/** Run whenever the connection becomes available. See `whenDaemonReachable`. */
const reachable = new Set<() => void>();

export const daemon = {
  get health() {
    return current;
  },
  /** What the strip says, or null when there is nothing to say. */
  get strip(): { kind: "warn" | "error"; text: string } | null {
    switch (current?.state) {
      case "reconnecting":
        return {
          kind: "warn",
          text: "The recorder is not running. Reconnecting… recordings in progress are unaffected.",
        };
      case "skewed":
        // **Deliberately not offering a button.** The two builds cannot agree,
        // and the one thing the UI must not do is tell the daemon to quit: it
        // may be recording.
        return {
          kind: "error",
          text:
            `This window is from a different build than the recorder ` +
            `(protocol ${current.ours} against ${current.theirs}). Restart the app.`,
        };
      default:
        return null;
    }
  },
};

export function initDaemonStatus() {
  // Outside Tauri there is no daemon and no pipe, and the mock transport
  // answers everything. Showing "the recorder is not running" against the
  // fixtures would be true and useless.
  if (!IN_TAURI) {
    // Said explicitly rather than left unknown, so `whenDaemonReachable` runs
    // its callers straight away in the browser. A frontend developer working
    // against the fixtures must not be made to wait on a handshake that will
    // never happen.
    apply({ state: "connected" });
    return;
  }

  // Asked once as well as subscribed, because a window that opens while the
  // daemon is already down would otherwise wait for the next change before
  // saying anything, and the next change may be minutes away.
  void daemonHealth()
    .then(apply)
    .catch(() => {});
  subscribe({ onHealth: apply });
}

/**
 * Runs `run` once the daemon is reachable, and again on every reconnect.
 *
 * Both halves are load-bearing. The first is the cold start: the window paints
 * before the handshake, so a fetch issued on load fails for a reason that is
 * not a failure. The second is recovery: `ui::client` reconnects and the strip
 * clears itself, but nothing re-read the library, so a window that lost its
 * daemon kept showing whatever it had when the connection died.
 *
 * Registering while already connected runs `run` immediately, so a caller
 * never has to ask which of the two cases it is in.
 */
export function whenDaemonReachable(run: () => void): void {
  reachable.add(run);
  if (current?.state === "connected") run();
}

function apply(health: DaemonHealth) {
  const cameBack = health.state === "connected" && current?.state !== "connected";
  current = health;
  if (cameBack) for (const run of reachable) run();
}
