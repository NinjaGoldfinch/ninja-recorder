/**
 * Escapes a string for interpolation into an `innerHTML` template.
 *
 * Everything in this app renders by building an HTML string and assigning
 * it, so every value that came from the backend has to pass through here
 * first — marker payloads carry other players' chosen names, and file
 * paths carry whatever the user named a folder.
 *
 * Lives in `shared/` because it was previously duplicated verbatim in
 * `main.ts` and `review.ts`, and the dev portal would have made three.
 */
export function escapeHtml(value: string): string {
  const div = document.createElement("div");
  div.textContent = value;
  return div.innerHTML;
}
