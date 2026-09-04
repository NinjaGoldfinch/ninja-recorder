import { el } from "./dom";

// Replaces the old `#status-msg` paragraph. That element sat above the
// library and carried both progress ("Rescanning…") and failures ("Failed
// to list recordings") — the failures being the reason it can't simply be
// deleted along with the layout it lived in.
let node: HTMLElement | null = null;
let timer: number | undefined;

export function initToast() {
  node = el("#toast");
}

export function toast(message: string, kind: "info" | "error" = "info") {
  if (!node) return;
  node.textContent = message;
  node.dataset.kind = kind;
  node.hidden = false;
  window.clearTimeout(timer);
  // Errors stay up longer: they're usually a sentence, and they're the ones
  // worth reading twice.
  timer = window.setTimeout(() => {
    if (node) node.hidden = true;
  }, kind === "error" ? 8000 : 4000);
}
