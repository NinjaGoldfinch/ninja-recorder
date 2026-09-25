/**
 * What the capture-backend row says. WS1.7, #11.
 *
 * Pure, so the wording is tested without mounting anything. Every string the
 * daemon supplies (a reason, the live backend's name) is rendered by
 * interpolation in `CaptureBackend.svelte`, never as markup.
 */

import type { CaptureBackend, CaptureBackendStatus } from "../contract/types";

/** How each backend is named in the control. */
export const BACKEND_LABELS: Record<CaptureBackend, string> = {
  libobs: "libobs",
  own: "Own",
};

/**
 * When a change applies, which is the part of this row most worth saying:
 * the daemon swaps the backend when nothing is recording, so it is the next
 * recording that uses it, and a game in progress is never switched under.
 */
export const APPLIES_WHEN =
  "Applies from the next recording. It can't be changed while a game is in progress.";

/** "Own isn't available: the own capture backend records on Windows only." */
export function unavailableNotes(status: CaptureBackendStatus): string[] {
  return status.options
    .filter((option) => option.unavailable !== null)
    .map((option) => `${BACKEND_LABELS[option.backend]} isn't available: ${option.unavailable}.`);
}

/**
 * The warning for a saved choice this build cannot construct, or `null`.
 *
 * The control cannot save one, but a downgrade or a missing libobs worker can
 * leave the daemon holding one, and then nothing is being recorded at all.
 * That is worth more than a disabled button to say.
 */
export function refusalNote(status: CaptureBackendStatus): string | null {
  const saved = status.options.find((option) => option.backend === status.configured);
  if (saved?.unavailable == null) return null;
  return (
    `Nothing will be recorded: ${BACKEND_LABELS[status.configured]} is selected ` +
    `and isn't available here (${saved.unavailable}). Choose one that is.`
  );
}
