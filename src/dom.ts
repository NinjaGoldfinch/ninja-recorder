// Required-element lookup. Throws rather than returning null: every
// caller here is wiring up markup that ships in the same repo, so a miss
// is a typo, and failing loudly at boot beats a silent no-op button.
export function el<T extends HTMLElement>(selector: string): T {
  const found = document.querySelector<T>(selector);
  if (!found) throw new Error(`Missing required element: ${selector}`);
  return found;
}

// Escapes text destined for a text node. Handles `<`, `>` and `&` — but
// NOT quotes, so it is not safe for attribute values. Use `escapeAttr`
// there.
export function escapeHtml(value: string): string {
  const div = document.createElement("div");
  div.textContent = value;
  return div.innerHTML;
}

// Escapes text destined for a quoted attribute value.
//
// This matters because recording titles are not all ours: `reconcile`
// imports any .mp4/.mkv dropped into the recordings folder, and the
// library falls back to the filename when a recording has no champion.
// A file named `x" onerror="…​.mp4` is inert in a text node and is not in
// an attribute.
export function escapeAttr(value: string): string {
  return escapeHtml(value).replace(/"/g, "&quot;").replace(/'/g, "&#39;");
}
