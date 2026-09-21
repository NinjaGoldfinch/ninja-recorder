// Vite's `?raw`, not `node:fs`: it resolves the same file the build does and
// needs no node types in a DOM tsconfig.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import indexHtml from "../index.html?raw";

/**
 * **The test that was missing.**
 *
 * WS4.4 deleted `#settings-view` from `index.html` and left
 * `registerView("settings", el("#settings-view"))` in `main.ts`. `el` throws
 * on a miss by design, and that line ran before `mountApp`, so the entire
 * frontend failed to boot: a window with static markup and no behaviour.
 *
 * It shipped green. No test imported `main.ts`, and the Windows smoke test
 * asserts the *process* reaches `setup` and connects, which is a claim about
 * Rust. Nothing anywhere asserted that the composition root survives contact
 * with the markup it composes against.
 *
 * So this boots the real `index.html` and the real `main.ts` together. It is
 * deliberately shallow: every module `main.ts` reaches is mocked down to
 * nothing, because what is under test is **the wiring**, not the modules. An
 * `el()` for an element the markup no longer has is exactly what it catches,
 * and that is a whole class of bug for the rest of WS4.
 */

const listen = vi.hoisted(() => vi.fn().mockResolvedValue(() => {}));
vi.mock("@tauri-apps/api/event", () => ({ listen }));
vi.mock("./bridge", () => ({
  call: vi.fn().mockResolvedValue(undefined),
  hasDevCommands: vi.fn().mockResolvedValue(false),
  assetUrl: (p: string) => p,
}));
vi.mock("./lib/stores/daemon.svelte", () => ({
  initDaemonStatus: vi.fn(),
  // Never fires: the point is to get through the synchronous boot, not to
  // exercise what happens after a handshake.
  whenDaemonReachable: vi.fn(),
  // `DaemonStrip.svelte` reads this. Null is "nothing to say", which is the
  // state a window that has not handshaken yet is in.
  daemon: { strip: null, health: undefined },
}));
vi.mock("./lib/stores/status.svelte", () => ({ initStatus: vi.fn(), stopStatusPolling: vi.fn() }));
vi.mock("./prefs", async (original) => ({
  ...(await original<typeof import("./prefs")>()),
  loadPrefs: vi.fn().mockResolvedValue({}),
}));

/** The shipped markup, not a fixture: a fixture would drift from it. */
function loadIndexHtml() {
  const body = indexHtml.slice(
    indexHtml.indexOf("<body>") + "<body>".length,
    indexHtml.indexOf("</body>"),
  );
  // The module scripts are loaded by the test, not by jsdom.
  document.body.innerHTML = body.replace(/<script[\s\S]*?<\/script>/g, "");
}

beforeEach(() => {
  vi.resetModules();
  loadIndexHtml();
});

afterEach(() => {
  document.body.innerHTML = "";
});

describe("booting the frontend", () => {
  it("gets through DOMContentLoaded against the real markup", async () => {
    await import("./main");

    // `main.ts` does all its work in this handler, so importing the module is
    // not enough: the failure was inside it.
    const errors: unknown[] = [];
    window.addEventListener("error", (e) => errors.push(e.error));
    window.dispatchEvent(new Event("DOMContentLoaded"));

    expect(errors).toEqual([]);
  });

  it("mounts the Svelte root, which is the thing the failure prevented", async () => {
    await import("./main");
    window.dispatchEvent(new Event("DOMContentLoaded"));

    const root = document.querySelector("#app-root");
    expect(root).not.toBeNull();
    // Every view is rendered by `App.svelte` now, so finding one proves the
    // mount happened rather than that the markup contains it.
    expect(root?.querySelector("#library-view")).not.toBeNull();
    expect(root?.querySelector("#review-view")).not.toBeNull();
    expect(root?.querySelector("#settings-view")).not.toBeNull();
  });

  it("finds every element it still looks up by id", async () => {
    // The specific shape of the bug: `el()` throws on a miss, and the markup
    // is being deleted a view at a time. Any `el("#...")` in the modules
    // `main.ts` boots must still match something.
    await import("./main");
    const thrown: unknown[] = [];
    window.addEventListener("error", (e) => thrown.push(e.error));
    window.dispatchEvent(new Event("DOMContentLoaded"));

    expect(thrown.filter((e) => String(e).includes("Missing required element"))).toEqual([]);
  });
});
