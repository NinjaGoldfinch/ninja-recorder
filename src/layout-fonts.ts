import regular from "../fixtures/fonts/selawk.woff2?url";
import bold from "../fixtures/fonts/selawkb.woff2?url";
import semibold from "../fixtures/fonts/selawksb.woff2?url";

/**
 * The font stack the layout gate (#345) measures with.
 *
 * Text width decides most of what those tests check, so the app's stack is
 * pinned: each family resolves to the real Segoe UI where it is installed
 * (Windows, which is what users run and what CI runs) and to Selawik, the
 * metric-compatible open font in `fixtures/fonts/`, where it is not.
 */
export async function useLayoutFonts(): Promise<void> {
  installFonts();
  await loadFonts();
}

function installFonts() {
  const faces: [string, string, number, string][] = [
    ["Segoe UI Variable Text", "Segoe UI Variable Text", 400, regular],
    ["Segoe UI Variable Text", "Segoe UI Variable Text Semibold", 600, semibold],
    ["Segoe UI Variable Text", "Segoe UI Variable Text Bold", 700, bold],
    ["Segoe UI", "Segoe UI", 400, regular],
    ["Segoe UI", "Segoe UI Semibold", 600, semibold],
    ["Segoe UI", "Segoe UI Bold", 700, bold],
  ];
  const style = document.createElement("style");
  style.textContent = faces
    .map(
      ([family, local, weight, url]) =>
        `@font-face{font-family:"${family}";font-weight:${weight};src:local("${local}"),url("${url}") format("woff2")}`,
    )
    .join("\n");
  document.head.append(style);
}

/** A font that fails its first fetch is never retried, and text then has no
 *  width at all; so ask until it answers. */
async function loadFonts() {
  for (const weight of [400, 600, 700]) {
    for (let attempt = 0; attempt < 10; attempt++) {
      try {
        if ((await document.fonts.load(`${weight} 14px "Segoe UI"`)).length > 0) break;
      } catch {
        // retried below
      }
      await new Promise((r) => setTimeout(r, 100));
    }
  }
  await document.fonts.ready;
}
