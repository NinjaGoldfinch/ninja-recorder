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
let views: Record<string, HTMLElement>;

function register(r: Router) {
  views = {
    library: document.createElement("div"),
    review: document.createElement("div"),
    settings: document.createElement("div"),
  };
  r.registerView("library", views.library);
  r.registerView("review", views.review);
  r.registerView("settings", views.settings);
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
  register(router);
});

afterEach(() => {
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
