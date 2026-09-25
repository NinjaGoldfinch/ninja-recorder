/**
 * The strip that says a recording lost something to a capture failure (#10).
 *
 * The daemon raises a desktop notification for the same thing, and that is
 * the part that reaches someone mid-game. This is the part that is still
 * there when they open the window: game audio refused by Windows, a microphone
 * that stopped part-way, a game that was not recorded at all. The same list is
 * stored with the recording, and the library row says it afterwards, so a
 * window that was closed when the event came still shows it there.
 *
 * ## Dismissed, not timed
 *
 * Like the recorder strip it sits beside, it does not go away on its own: it
 * is a fact about the last recording, and one that vanished before anyone
 * read it would be as good as not having said it. Unlike that strip, it is
 * not a *state* that clears itself, so it has a dismiss button. The next
 * problem replaces it.
 */

import { type CaptureNotice, type CaptureProblemsEvent, noticeFor } from "../library/problems";
import { IN_TAURI } from "../transport/invoke";
import { subscribe } from "../transport/pipe";

let current = $state<CaptureNotice | null>(null);

export const captureNotice = {
  /** What the strip says, or null when there is nothing to say. */
  get notice(): CaptureNotice | null {
    return current;
  },
};

/** Shows what one `captureProblems` event says, replacing what was there. */
export function showCaptureProblems(event: CaptureProblemsEvent) {
  const notice = noticeFor(event);
  if (notice) current = notice;
}

export function dismissCaptureNotice() {
  current = null;
}

/**
 * Listens for the event. Outside Tauri there is no daemon to raise one, and
 * the mock transport pushes nothing.
 */
export function initCaptureNotices() {
  if (!IN_TAURI) return;
  subscribe({
    onEvent: (event) => {
      if (event.type === "captureProblems") showCaptureProblems(event);
    },
  });
}
