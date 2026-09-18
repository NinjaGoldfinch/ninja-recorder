import type { Component } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

/**
 * `router.ts` — WS5 task 5.5, and the module WS4 has to keep honest.
 *
 * It is small, but it holds one invariant nothing else does: **the router's
 * idea of which view is showing and the DOM's `hidden` attributes must not be
 * able to disagree.** The review player's document-level hotkeys are gated on
 * the current view, so a disagreement means `[` and `]` seeking a video
 * nobody can see. That is the property most of the tests below are about.
 *
 * `@tauri-apps/api/event` is mocked rather than stubbed at the window: it is
 * the transport seam WS2 replaces, so a test that reached through it would
 * have to be rewritten twice.
 */

const listen = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/event", () => ({ listen }));

type Router = typeof import("./router");

let router: Router;
/**
 * Imported per test, not at the top of the file.
 *
 * `vi.resetModules` below gives each test a fresh `router`, and with it a
 * fresh copy of the Svelte runtime that `mount` comes from. A statically
 * imported component would still be compiled against the *first* copy, and
 * its `$effect` would then run with no active effect context: `effect_orphan`,
 * reported against `App.svelte` with nothing to say about why.
 */
let App: Component;
let views: Record<string, HTMLElement>;
let host: HTMLElement;

function register(r: Router) {
  views = {
    library: document.createElement("div"),
    review: document.createElement("div"),
    settings: document.createElement("div"),
  };
  r.registerView("library", views.library);
  r.registerView("review", views.review);
  r.registerView("settings", views.settings);

  // Where the Svelte root goes. In the app this is `#svelte-root`, the last
  // child of `.container`; here it only has to be a node the vanilla views
  // are not inside, which is the whole of what "beside them" means.
  host = document.createElement("div");
  document.body.append(host);
}

/** Which views the DOM says are showing, as against what the router says. */
function visible(): string[] {
  return Object.entries(views)
    .filter(([, node]) => !node.hidden)
    .map(([name]) => name);
}

beforeEach(async () => {
  // The module holds `current` in module scope, so every test needs its own
  // copy — otherwise the second test starts wherever the first one left off.
  vi.resetModules();
  listen.mockReset();
  listen.mockResolvedValue(() => {});
  window.location.hash = "";
  router = await import("./router");
  App = (await import("./lib/App.svelte")).default;
  register(router);
});

afterEach(async () => {
  // The root is module state, like `current`: a test that leaves it up would
  // make the next `mountApp` a no-op and the failure would surface somewhere
  // else entirely.
  await router.unmountApp();
  host.remove();
  window.location.hash = "";
});

describe("showView", () => {
  it("leaves exactly one view showing", () => {
    router.showView("settings");
    expect(visible()).toEqual(["settings"]);
    expect(router.currentView()).toBe("settings");
  });

  it("keeps the router and the DOM agreeing after several moves", () => {
    router.showView("settings");
    router.showView("review");
    router.showView("library");
    expect(visible()).toEqual(["library"]);
    expect(router.currentView()).toBe("library");
  });

  it("starts on the library", () => {
    expect(router.currentView()).toBe("library");
  });
});

describe("registerView", () => {
  it("hides a view that registers after the router has moved on", () => {
    // WS4.3: a migrated view registers itself when its component mounts, which
    // is after `initRouting` has read the URL fragment. A window opened at
    // `#settings` would otherwise show the settings section and the library
    // at once, because the library's node was not there for the `showView`
    // that hid everything else.
    router.showView("settings");

    const late = document.createElement("div");
    router.registerView("library", late);
    expect(late.hidden).toBe(true);
    expect(router.currentView()).toBe("settings");
  });

  it("shows a view that registers while it is the current one", () => {
    const late = document.createElement("div");
    late.hidden = true;
    router.registerView("library", late);
    expect(late.hidden).toBe(false);
  });
});

describe("onViewChange", () => {
  it("reports every move, once each", () => {
    const seen: string[] = [];
    router.onViewChange((v) => seen.push(v));
    router.showView("settings");
    router.showView("review");
    expect(seen).toEqual(["settings", "review"]);
  });

  it("says nothing when the view did not change", () => {
    // Load-bearing: the review player tears down and rebuilds on this
    // callback, so a redundant fire would restart the video.
    const seen: string[] = [];
    router.onViewChange((v) => seen.push(v));
    router.showView("settings");
    router.showView("settings");
    expect(seen).toEqual(["settings"]);
  });

  it("notifies every listener, not just the first", () => {
    const a: string[] = [];
    const b: string[] = [];
    router.onViewChange((v) => a.push(v));
    router.onViewChange((v) => b.push(v));
    router.showView("review");
    expect(a).toEqual(["review"]);
    expect(b).toEqual(["review"]);
  });
});

describe("initRouting", () => {
  it("opens on the view named in the fragment", () => {
    // The tray's Settings item opens `index.html#settings` when no window
    // exists yet, because the frontend cannot listen for an event it has not
    // loaded to subscribe to.
    window.location.hash = "#settings";
    router.initRouting();
    expect(router.currentView()).toBe("settings");
    expect(visible()).toEqual(["settings"]);
  });

  it("refuses to open straight into the review player", () => {
    // `#review` has no recording selected, so honouring it would show an
    // empty player instead of the library.
    window.location.hash = "#review";
    router.initRouting();
    expect(router.currentView()).toBe("library");
  });

  it("ignores a fragment that is not a view", () => {
    window.location.hash = "#not-a-view";
    router.initRouting();
    expect(router.currentView()).toBe("library");
  });

  it("subscribes to the tray's navigate event", () => {
    router.initRouting();
    expect(listen).toHaveBeenCalledWith("navigate", expect.any(Function));
  });

  it("moves when the tray navigates a window that already exists", () => {
    router.initRouting();
    const handler = listen.mock.calls[0][1] as (e: { payload: string }) => void;
    handler({ payload: "settings" });
    expect(router.currentView()).toBe("settings");
    handler({ payload: "nonsense" });
    expect(router.currentView()).toBe("settings");
  });

  it("starts anyway when the event bridge is unavailable", () => {
    // A browser tab with no Tauri behind it is the dev-server case, and a
    // rejected `listen` must not take the whole frontend down with it.
    listen.mockRejectedValue(new Error("no tauri"));
    expect(() => router.initRouting()).not.toThrow();
  });
});

describe("the Svelte root", () => {
  /**
   * WS4.1's exit criterion, and the only claim this task makes: an empty
   * `App.svelte` goes up and comes down again without the vanilla views
   * noticing. Everything WS4.2 onwards does is moving markup across this
   * seam, so a break here is a break in all of it.
   */

  it("mounts into its host", async () => {
    expect(router.appMounted()).toBe(false);
    router.mountApp(App, host);
    expect(router.appMounted()).toBe(true);
  });

  it("unmounts again, and says so", async () => {
    router.mountApp(App, host);
    await expect(router.unmountApp()).resolves.toBe(true);
    expect(router.appMounted()).toBe(false);
  });

  it("reports nothing to unmount when it was never up", async () => {
    await expect(router.unmountApp()).resolves.toBe(false);
  });

  it("refuses to mount a second copy over the first", () => {
    // A second root in the same node duplicates the UI rather than throwing,
    // which is the kind of bug that gets diagnosed as a CSS problem. Counting
    // the library sections is what makes this able to fail: a second mount
    // would render a second one into the same host.
    router.mountApp(App, host);
    expect(host.querySelectorAll("#library-view")).toHaveLength(1);

    router.mountApp(App, host);
    expect(router.appMounted()).toBe(true);
    expect(host.querySelectorAll("#library-view")).toHaveLength(1);
  });

  it("renders the views that have migrated", () => {
    // WS4.1 asserted the opposite, that the root rendered nothing, and said
    // WS4.3 would be the commit to change it. This is that commit: the
    // library is a Svelte view now, and `library.ts` is gone.
    router.mountApp(App, host);
    expect(host.querySelector("#library-view")).not.toBeNull();
  });

  it("can go up, come down and go up again", async () => {
    router.mountApp(App, host);
    await router.unmountApp();
    router.mountApp(App, host);
    expect(router.appMounted()).toBe(true);
  });

  it("leaves the vanilla views exactly as it found them", async () => {
    router.showView("settings");
    const before = visible();

    router.mountApp(App, host);
    expect(visible()).toEqual(before);
    expect(router.currentView()).toBe("settings");

    await router.unmountApp();
    expect(visible()).toEqual(before);
    expect(router.currentView()).toBe("settings");
  });

  it("does not become a view the router can switch to", () => {
    // The host is not registered, so `showView` must not touch it. Once a
    // migrated view lives in there, WS4.3 registers the host itself and this
    // is the line that changes.
    router.mountApp(App, host);
    router.showView("review");
    expect(host.hidden).toBe(false);
  });
});
