import type { Component } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

/**
 * The title bar: the view tabs, the client pill and its card, the settings
 * button and its badge, the dev portal button that is only there in a build
 * that carries the commands, and the window controls.
 */

const call = vi.hoisted(() => vi.fn());
const hasDevCommands = vi.hoisted(() => vi.fn());
vi.mock("../../../bridge", () => ({ call, hasDevCommands, assetUrl: (p: string) => p }));
const showView = vi.hoisted(() => vi.fn());
const viewListeners = vi.hoisted(() => [] as ((view: string) => void)[]);
vi.mock("../../../router", () => ({
  showView,
  currentView: () => "library",
  onViewChange: (cb: (view: string) => void) => {
    viewListeners.push(cb);
    return () => viewListeners.splice(viewListeners.indexOf(cb), 1);
  },
  registerView: vi.fn(),
  initRouting: vi.fn(),
}));
const daemonState = vi.hoisted(() => ({ health: { state: "connected" } as { state: string } }));
vi.mock("../../stores/daemon.svelte", () => ({
  daemon: daemonState,
  whenDaemonReachable: (run: () => void) => run(),
}));

// The window: a real one only inside Tauri, so the tests say which they are.
const tauri = vi.hoisted(() => ({ inTauri: true }));
vi.mock("../../transport/invoke", () => ({
  get IN_TAURI() {
    return tauri.inTauri;
  },
}));
const win = vi.hoisted(() => ({
  minimize: vi.fn(),
  toggleMaximize: vi.fn(),
  close: vi.fn(),
  isMaximized: vi.fn(),
  onResized: vi.fn(),
}));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => win }));

let host: HTMLElement;
let instance: Record<string, unknown> | null = null;
let AppBar: Component;
let client: typeof import("../../stores/client.svelte");
let update: typeof import("../../stores/update.svelte");
type Svelte = typeof import("svelte");
let svelte: Svelte;

beforeEach(async () => {
  vi.resetModules();
  call.mockReset();
  showView.mockReset();
  hasDevCommands.mockReset().mockResolvedValue(false);
  viewListeners.length = 0;
  tauri.inTauri = true;
  daemonState.health = { state: "connected" };
  win.minimize.mockReset();
  win.toggleMaximize.mockReset();
  win.close.mockReset();
  win.isMaximized.mockReset().mockResolvedValue(false);
  win.onResized.mockReset().mockResolvedValue(() => {});

  svelte = await import("svelte");
  client = await import("../../stores/client.svelte");
  update = await import("../../stores/update.svelte");
  AppBar = (await import("./AppBar.svelte")).default;

  host = document.createElement("div");
  document.body.append(host);
});

afterEach(async () => {
  if (instance) await svelte.unmount(instance, { outro: false });
  host.remove();
  instance = null;
});

async function render() {
  instance = svelte.mount(AppBar, { target: host });
  await Promise.resolve();
  svelte.flushSync();
  return host;
}

const flushSync = () => svelte.flushSync();

const lcu = (over: Record<string, unknown> = {}) => ({
  connected: true,
  phase: "Lobby",
  summoner: "NinjaGoldfinch#OCE",
  profile_icon_id: null,
  error: null,
  ...over,
});

const supervisor = (state: string, elapsed: number | null = null) =>
  ({ state, recording_elapsed_s: elapsed, last_finalized: null }) as never;

describe("the client pill", () => {
  it("starts by saying it is still asking", async () => {
    // "Not known yet" is not "not running", and the pill must not claim the
    // client is absent before anything has looked.
    const el = await render();
    expect(el.querySelector(".client-pill")?.textContent).toContain("Checking client");
  });

  it("words the phase, never the raw value, and colours on the tone", async () => {
    const el = await render();
    client.setClientLcu(lcu({ phase: "ChampSelect" }));
    client.setClientGame(supervisor("WaitingForGame"));
    flushSync();

    const pill = el.querySelector(".client-pill");
    expect(pill?.textContent).toContain("Champion select");
    expect(pill?.textContent).not.toContain("ChampSelect");
    expect(pill?.getAttribute("data-tone")).toBe("active");
  });

  it("shows the recording clock while recording", async () => {
    const el = await render();
    client.setClientLcu(lcu({ phase: "InProgress" }));
    client.setClientGame(supervisor("Recording", 872));
    flushSync();

    const pill = el.querySelector(".client-pill");
    expect(pill?.getAttribute("data-tone")).toBe("recording");
    expect(pill?.textContent).toContain("14:32");
  });
});

describe("the client card", () => {
  const card = (el: HTMLElement) => el.querySelector<HTMLElement>(".client-card");

  it("is closed until the pill is hovered, and opens on hover", async () => {
    const el = await render();
    expect(card(el)?.hidden).toBe(true);

    el.querySelector(".client-status")?.dispatchEvent(new MouseEvent("mouseenter"));
    flushSync();
    expect(card(el)?.hidden).toBe(false);
    expect(el.querySelector(".client-pill")?.getAttribute("aria-expanded")).toBe("true");
  });

  it("pins open on click and closes on Escape", async () => {
    const el = await render();
    el.querySelector<HTMLButtonElement>(".client-pill")?.click();
    flushSync();
    expect(card(el)?.hidden).toBe(false);

    el.querySelector(".client-status")?.dispatchEvent(
      new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
    );
    flushSync();
    expect(card(el)?.hidden).toBe(true);
  });

  it("shows the Riot ID with its tag apart, the phase, and a Ready recorder", async () => {
    // An unread capture status is not a problem; the card only says
    // something else when there is something wrong.
    call.mockImplementation((name: string) => Promise.reject(new Error(`${name} not mocked`)));
    const el = await render();
    client.setClientLcu(lcu({ phase: "Matchmaking" }));
    client.setClientGame(supervisor("ClientRunning"));
    flushSync();

    expect(el.querySelector(".client-name")?.textContent).toBe("NinjaGoldfinch#OCE");
    expect(el.querySelector(".client-tag")?.textContent).toBe("#OCE");
    expect(card(el)?.textContent).toContain("Searching for a match");
    expect(el.querySelector(".client-chip")?.textContent).toBe("Ready");
    expect(el.querySelector('.client-track-bar [data-at="now"]')).not.toBeNull();
  });

  it("draws the profile icon once it resolves, and an initial until then", async () => {
    call.mockImplementation((name: string) =>
      name === "resolve_icons"
        ? Promise.resolve({ profile_icons: { 29: "C:/icons/29.png" } })
        : Promise.reject(new Error("not mocked")),
    );
    const el = await render();
    client.setClientLcu(lcu({ profile_icon_id: 29 }));
    flushSync();
    expect(el.querySelector(".client-avatar")?.textContent?.trim()).toBe("N");

    for (let i = 0; i < 6; i++) await Promise.resolve();
    flushSync();
    expect(el.querySelector<HTMLImageElement>(".client-avatar img")?.src).toContain("29.png");
    expect(call).toHaveBeenCalledWith("resolve_icons", { request: { profileIcons: [29] } });
  });

  it("says the recorder is not running when the daemon is away", async () => {
    daemonState.health = { state: "reconnecting" };
    const el = await render();
    expect(el.querySelector(".client-chip")?.textContent).toBe("Not running");
  });
});

describe("the tabs", () => {
  const current = (el: HTMLElement) =>
    el.querySelector('.app-tab[aria-current="page"]')?.textContent?.trim();

  it("marks the library, and keeps it marked in the review it opened", async () => {
    const el = await render();
    expect(current(el)).toBe("Library");

    for (const cb of viewListeners) cb("review");
    flushSync();
    expect(current(el)).toBe("Library");

    for (const cb of viewListeners) cb("objectives");
    flushSync();
    expect(current(el)).toBe("Objectives");
  });

  it("switches views", async () => {
    const el = await render();
    const tabs = el.querySelectorAll<HTMLButtonElement>(".app-tab");
    tabs[1].click();
    expect(showView).toHaveBeenCalledWith("objectives");
    tabs[0].click();
    expect(showView).toHaveBeenCalledWith("library");
  });
});

describe("the window controls", () => {
  it("minimise, maximise and close the window", async () => {
    // Close is `close()`, which raises CloseRequested: close-to-tray and the
    // quit prompt live behind it, as they did behind the native button.
    const el = await render();
    el.querySelector<HTMLButtonElement>('[aria-label="Minimise"]')?.click();
    el.querySelector<HTMLButtonElement>('[aria-label="Maximise"]')?.click();
    el.querySelector<HTMLButtonElement>('[aria-label="Close"]')?.click();
    expect(win.minimize).toHaveBeenCalled();
    expect(win.toggleMaximize).toHaveBeenCalled();
    expect(win.close).toHaveBeenCalled();
  });

  it("offers Restore when the window is maximised", async () => {
    win.isMaximized.mockResolvedValue(true);
    const el = await render();
    await Promise.resolve();
    flushSync();
    expect(el.querySelector('[aria-label="Restore"]')).not.toBeNull();
  });

  it("are left out where there is no window to drive", async () => {
    tauri.inTauri = false;
    const el = await render();
    expect(el.querySelector(".window-controls")).toBeNull();
  });

  it("make the bar a drag region that its buttons punch through", async () => {
    // Tauri's drag script treats a button as a hole in a `deep` region.
    const el = await render();
    expect(el.querySelector("header")?.getAttribute("data-tauri-drag-region")).toBe("deep");
  });
});

describe("the update badge", () => {
  it("is absent until an update is offered", async () => {
    const el = await render();
    expect(el.querySelector(".badge")).toBeNull();
  });

  it("appears when one is, and says so for a screen reader", async () => {
    // **The entire announcement.** Deliberately not a toast: CI releases every
    // commit on main, so it would fire most days.
    call.mockImplementation((name: string) =>
      name === "get_update_status"
        ? Promise.resolve({
            kind: "available",
            installable: true,
            offer: { version: "2.1.0", notes: null },
          })
        : Promise.reject(new Error(`${name} not mocked`)),
    );
    const el = await render();
    await update.refreshUpdateStatus();
    await Promise.resolve();

    expect(el.querySelector(".badge")).not.toBeNull();
    expect(el.textContent).toContain("An update is available");
  });
});

describe("the buttons", () => {
  it("opens settings", async () => {
    const el = await render();
    el.querySelector<HTMLButtonElement>('[aria-label="Settings"]')?.click();
    expect(showView).toHaveBeenCalledWith("settings");
  });

  it("hides the dev portal button in a build without the commands", async () => {
    // Detected rather than configured: a rejection is the expected outcome in
    // a shipped build, which is why `hasDevCommands` answers false.
    const el = await render();
    expect(el.querySelector('[aria-label="Dev portal"]')).toBeNull();
  });

  it("shows it in a build with them, and opens the portal", async () => {
    hasDevCommands.mockResolvedValue(true);
    const el = await render();
    await Promise.resolve();

    const button = el.querySelector<HTMLButtonElement>('[aria-label="Dev portal"]');
    expect(button).not.toBeNull();

    call.mockResolvedValue(undefined);
    button?.click();
    expect(call).toHaveBeenCalledWith("dev_open_portal");
  });

  it("takes the button away if opening the portal fails", async () => {
    // The probe said yes and the command then refused, which means the answer
    // has changed under us; leaving a button that does nothing is worse.
    hasDevCommands.mockResolvedValue(true);
    const el = await render();
    await Promise.resolve();

    call.mockRejectedValue(new Error("not registered"));
    el.querySelector<HTMLButtonElement>('[aria-label="Dev portal"]')?.click();
    await Promise.resolve();
    await Promise.resolve();

    expect(el.querySelector('[aria-label="Dev portal"]')).toBeNull();
  });
});
