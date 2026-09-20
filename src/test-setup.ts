/**
 * What jsdom does not implement but the app needs at import time.
 *
 * Kept to genuine environment gaps. Anything the app could reasonably be asked
 * to tolerate belongs in the app, not here: a shim that papers over a real
 * fragility is a test that stops describing the product.
 */

/**
 * `matchMedia` has no jsdom implementation, and `theme.ts` calls it at module
 * scope: `const media = window.matchMedia("(prefers-color-scheme: dark)")`.
 *
 * That call is load-bearing and must not move. It is the only thing making the
 * "System" theme follow the OS as it changes, and CLAUDE.md names removing its
 * `change` listener as a silent regression with no test to catch it. So the
 * environment is what gets fixed.
 *
 * `matches: false` means "the OS is in light mode", which is a definite answer
 * rather than a broken one, and `addEventListener` is a no-op because nothing
 * under test drives an OS theme change.
 */
if (typeof window !== "undefined" && typeof window.matchMedia !== "function") {
  window.matchMedia = ((query: string) => ({
    matches: false,
    media: query,
    onchange: null,
    addEventListener: () => {},
    removeEventListener: () => {},
    addListener: () => {},
    removeListener: () => {},
    dispatchEvent: () => false,
  })) as typeof window.matchMedia;
}
