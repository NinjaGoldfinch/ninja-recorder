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
 * **It never fires on its own**, and it must not: an element's width in jsdom
 * is zero, so a stub that invented a callback would be inventing a layout, and
 * every test would then be asserting against a number nobody chose.
 *
 * `resizeTo` is the way in. A test that needs a laid-out width says so, in the
 * test, with the number visible beside the assertion that depends on it. That
 * is the difference between stating an assumption and hiding one, and it is
 * what makes the marker glyphs reachable at all: `clusterMarkers` measures in
 * pixels and correctly returns nothing at zero width.
 */
interface Watcher {
  callback: ResizeObserverCallback;
  targets: Set<Element>;
}

const watchers = new Set<Watcher>();

if (typeof globalThis.ResizeObserver !== "function") {
  globalThis.ResizeObserver = class {
    #watcher: Watcher;
    constructor(callback: ResizeObserverCallback) {
      this.#watcher = { callback, targets: new Set() };
      watchers.add(this.#watcher);
    }
    observe(target: Element) {
      this.#watcher.targets.add(target);
    }
    unobserve(target: Element) {
      this.#watcher.targets.delete(target);
    }
    disconnect() {
      watchers.delete(this.#watcher);
    }
  } as unknown as typeof ResizeObserver;
}

/**
 * Gives an element a width and tells whoever is observing it.
 *
 * Svelte's `bind:clientWidth` reads the property back when the observer fires,
 * so both halves are needed: the property is what the binding sees, and the
 * callback is what makes it look.
 */
export function resizeTo(target: Element, width: number, height = 100): void {
  Object.defineProperty(target, "clientWidth", { value: width, configurable: true });
  Object.defineProperty(target, "clientHeight", { value: height, configurable: true });
  const entry = { target, contentRect: { width, height } } as unknown as ResizeObserverEntry;
  for (const watcher of watchers) {
    if (watcher.targets.has(target)) watcher.callback([entry], null as never);
  }
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

/**
 * jsdom parses `<dialog>` but implements neither `showModal` nor `close`.
 *
 * `QuitDialog.svelte` is built on both, and on the `close` event carrying a
 * `returnValue`, which is what lets Escape and the two buttons resolve the
 * same promise with different answers. The shim keeps that contract: `close`
 * records the value, drops `open`, and fires the event.
 *
 * Nothing here makes it modal. The focus trap is the browser's job and no test
 * asserts on it.
 */
if (typeof window !== "undefined" && typeof HTMLDialogElement !== "undefined") {
  // **Overridden unconditionally, not behind a `typeof` check.** jsdom
  // *defines* `showModal` and then throws "not implemented" from it, so a
  // guard that asks whether the method exists installs nothing and the dialog
  // silently never opens.
  const dialog = HTMLDialogElement.prototype as unknown as Record<string, unknown>;
  dialog.showModal = function showModal(this: HTMLDialogElement) {
    this.setAttribute("open", "");
  };
  dialog.show = dialog.showModal;
  dialog.close = function close(this: HTMLDialogElement, returnValue?: string) {
    if (returnValue !== undefined) this.returnValue = returnValue;
    if (!this.hasAttribute("open")) return;
    this.removeAttribute("open");
    this.dispatchEvent(new Event("close"));
  };
}
