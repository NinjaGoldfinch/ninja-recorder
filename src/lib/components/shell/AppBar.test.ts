import type { Component } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

/**
 * The header: the two pills, the settings button and its badge, and the dev
 * portal button that is only there in a build that carries the commands.
 */

const call = vi.hoisted(() => vi.fn());
const hasDevCommands = vi.hoisted(() => vi.fn());
vi.mock("../../../bridge", () => ({ call, hasDevCommands, assetUrl: (p: string) => p }));
const showView = vi.hoisted(() => vi.fn());
vi.mock("../../../router", () => ({
  showView,
  currentView: () => "library",
  onViewChange: vi.fn(),
  registerView: vi.fn(),
  initRouting: vi.fn(),
}));

let host: HTMLElement;
let instance: Record<string, unknown> | null = null;
let AppBar: Component;
let about: typeof import("../../stores/about.svelte");
let update: typeof import("../../stores/update.svelte");
type Svelte = typeof import("svelte");
let svelte: Svelte;

beforeEach(async () => {
  vi.resetModules();
  call.mockReset();
  showView.mockReset();
  hasDevCommands.mockReset().mockResolvedValue(false);

  svelte = await import("svelte");
  about = await import("../../stores/about.svelte");
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
  return host;
}

describe("the status pills", () => {
  it("starts by saying it is still asking", async () => {
    // "Not known yet" is not "not running", and the pill must not claim the
    // client is absent before anything has looked.
    const el = await render();
    expect(el.textContent).toContain("Checking client");
  });

  it("shows what the poll reports, and colours on the state", async () => {
    const el = await render();
    about.setLcuPill({ state: "online", copy: "Ninja" });
    about.setGamePill({ state: "recording", copy: "Recording" });
    await Promise.resolve();

    expect(el.textContent).toContain("Ninja");
    expect(el.querySelector('[data-state="recording"]')).not.toBeNull();
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
    call.mockResolvedValue({
      kind: "available",
      installable: true,
      offer: { version: "2.1.0", notes: null },
    });
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
