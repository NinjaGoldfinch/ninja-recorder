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

/**
 * `ResizeObserver` has no jsdom implementation either, and Svelte uses one to
 * back `bind:clientWidth`, which `Timeline.svelte` needs because marker
 * clusters are measured in pixels.
 *
 * It never fires. Nothing under test resizes anything, and a stub that
 * invented a callback would be inventing a layout: an element's width in jsdom
 * is zero, and `clusterMarkers` correctly returns nothing at zero width. Tests
 * that care about clustering call it directly.
 */
if (typeof globalThis.ResizeObserver !== "function") {
  globalThis.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver;
}

/**
 * jsdom parses `<video>` but implements none of its playback: `play`, `pause`
 * and `load` throw "Not implemented", and `duration` is always `NaN`.
 *
 * These make the element inert rather than pretending it plays. `play`
 * resolves and flips `paused`, because the component listens for the events
 * the real element fires and a rejected promise would be a different test.
 * Nothing here simulates time passing; a test that needs a position sets
 * `currentTime` itself.
 */
if (typeof window !== "undefined" && typeof HTMLMediaElement !== "undefined") {
  // **Per element, not per module.** A shared flag leaks between elements and
  // between tests: a component that played in one test would start the next
  // one already playing, and the play button would render as pause.
  const playing = new WeakSet<HTMLMediaElement>();
  const media = HTMLMediaElement.prototype as unknown as Record<string, unknown>;

  Object.defineProperty(media, "paused", {
    get(this: HTMLMediaElement) {
      return !playing.has(this);
    },
    configurable: true,
  });
  media.play = function play(this: HTMLMediaElement) {
    playing.add(this);
    this.dispatchEvent(new Event("play"));
    return Promise.resolve();
  };
  media.pause = function pause(this: HTMLMediaElement) {
    playing.delete(this);
    this.dispatchEvent(new Event("pause"));
  };
  media.load = () => {};
}
