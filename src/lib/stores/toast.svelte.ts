/**
 * The transient message - WS4 task 4.6.
 *
 * Replaces `toast.ts`, which owned `#toast` and wrote into it. The timer and
 * the two durations are unchanged; what has gone is the element reference and
 * the `initToast` that had to be called before the first failure could be
 * reported.
 *
 * It replaced the old `#status-msg` paragraph, which sat above the library and
 * carried both progress ("Rescanning...") and failures ("Failed to list
 * recordings"). The failures are the reason it cannot simply be deleted along
 * with the layout it lived in.
 */

export type ToastKind = "info" | "error";

let message = $state<string | null>(null);
let kind = $state<ToastKind>("info");
let timer: ReturnType<typeof setTimeout> | undefined;

export const toastState = {
  get message() {
    return message;
  },
  get kind() {
    return kind;
  },
};

export function toast(next: string, nextKind: ToastKind = "info") {
  message = next;
  kind = nextKind;
  clearTimeout(timer);
  // **Errors stay up longer**: they are usually a sentence, and they are the
  // ones worth reading twice.
  timer = setTimeout(
    () => {
      message = null;
    },
    nextKind === "error" ? 8000 : 4000,
  );
}

/** Dismisses whatever is showing. For teardown, and for tests. */
export function clearToast() {
  clearTimeout(timer);
  message = null;
}
