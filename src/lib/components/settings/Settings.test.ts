import type { Component } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

/**
 * The settings view, against the behaviours that were easiest to get wrong in
 * `settings.ts` because they were spread across an element map, a `syncX`
 * function and a listener.
 *
 * The bridge is mocked: it is the transport seam WS2 replaced, and every
 * control here is either a preference write or an RPC.
 */

const call = vi.hoisted(() => vi.fn());
vi.mock("../../../bridge", () => ({
  call,
  hasDevCommands: vi.fn().mockResolvedValue(false),
  assetUrl: (p: string) => p,
}));
// `whenDaemonReachable` runs its callback once the handshake lands. The view
// uses it to gate four RPCs; here it fires immediately so those paths run.
vi.mock("../../stores/daemon.svelte", () => ({
  whenDaemonReachable: (fn: () => void) => fn(),
  initDaemonStatus: vi.fn(),
}));

let host: HTMLElement;
let instance: Record<string, unknown> | null = null;
let Settings: Component;
let store: typeof import("../../stores/settings.svelte");
type Svelte = typeof import("svelte");
let svelte: Svelte;

/** Answers each RPC the view makes on mount with something plausible. */
function stubBackend(over: Record<string, unknown> = {}) {
  const answers: Record<string, unknown> = {
    get_autostart: { enabled: false, supported: true },
    get_retention_policy: { max_total_bytes: null, max_age_days: null },
    get_recordings_dir: "C:/vods",
    list_audio_inputs: [],
    get_audio_preset: { preset: "game" },
    get_update_status: { kind: "upToDate" },
    ...over,
  };
  call.mockImplementation((name: string) =>
    name in answers ? Promise.resolve(answers[name]) : Promise.resolve(undefined),
  );
}

beforeEach(async () => {
  vi.resetModules();
  call.mockReset();
  stubBackend();

  svelte = await import("svelte");
  store = await import("../../stores/settings.svelte");
  Settings = (await import("./Settings.svelte")).default;

  host = document.createElement("div");
  document.body.append(host);
});

afterEach(async () => {
  if (instance) await svelte.unmount(instance, { outro: false });
  host.remove();
  instance = null;
});

function render(): HTMLElement {
  instance = svelte.mount(Settings, { target: host });
  return host;
}

/** Lets the mount-time RPCs settle. */
const settle = () => new Promise((r) => setTimeout(r, 0));

describe("the notification switches", () => {
  it("disables the three events when the master switch is off", async () => {
    // **The master switch gates the rest in Rust**, so the form has to say so
    // rather than leaving three checkboxes that look live and do nothing.
    store.setPref("notifications", "off");
    const el = render();
    await settle();

    const started = el.querySelector<HTMLInputElement>(
      '[aria-label="Notify when recording starts"]',
    );
    expect(started?.disabled).toBe(true);
  });

  it("enables them when it is on", async () => {
    store.setPref("notifications", "on");
    const el = render();
    await settle();
    expect(
      el.querySelector<HTMLInputElement>('[aria-label="Notify when recording starts"]')?.disabled,
    ).toBe(false);
  });
});

describe("start on login", () => {
  it("reflects what the platform says, not what was asked for", async () => {
    // A Run-key write can be overruled by policy, so the response is what the
    // checkbox follows.
    stubBackend({ get_autostart: { enabled: true, supported: true } });
    const el = render();
    await settle();
    expect(el.querySelector<HTMLInputElement>('[aria-label="Start on login"]')?.checked).toBe(true);
  });

  it("says so, and disables the control, when the build does not support it", async () => {
    stubBackend({ get_autostart: { enabled: false, supported: false } });
    const el = render();
    await settle();
    expect(el.textContent).toContain("Not available in this build.");
    expect(el.querySelector<HTMLInputElement>('[aria-label="Start on login"]')?.disabled).toBe(
      true,
    );
  });

  it("reports a failed read rather than claiming the app does not start on login", async () => {
    // Leaving the box unticked would be a claim the read did not support.
    // Only this one fails. Blanking every other answer would be testing a
    // backend that does not exist.
    stubBackend();
    const answers = call.getMockImplementation();
    call.mockImplementation((name: string, ...rest: unknown[]) =>
      name === "get_autostart"
        ? Promise.reject(new Error("not connected"))
        : (answers as (n: string, ...r: unknown[]) => unknown)(name, ...rest),
    );
    const el = render();
    await settle();
    expect(el.textContent).toContain("Couldn't read this setting");
  });
});

describe("the audio panel", () => {
  it("enables the microphone picker only for presets that record one", async () => {
    stubBackend({ get_audio_preset: { preset: "game" } });
    const el = render();
    await settle();
    expect(el.querySelector<HTMLSelectElement>('[aria-label="Microphone"]')?.disabled).toBe(true);

    await store.saveAudioPreset("game_mic");
    await settle();
    expect(el.querySelector<HTMLSelectElement>('[aria-label="Microphone"]')?.disabled).toBe(false);
  });

  it("shows the tracks the chosen preset produces", async () => {
    stubBackend({ get_audio_preset: { preset: "game_mic" } });
    const el = render();
    await settle();
    expect(el.textContent).toContain("Track 0: Everything");
    expect(el.textContent).toContain("Track 2: Mic");
  });

  it("falls back to a known preset rather than showing nothing", async () => {
    // `custom` has no button, and a newer build's settings row can carry a
    // value this one has never heard of.
    stubBackend({ get_audio_preset: { preset: "custom" } });
    render();
    await settle();
    expect(store.settings.audioPreset).toBe("game");
  });
});

describe("the retention form", () => {
  it("loads the saved policy into the form", async () => {
    stubBackend({
      get_retention_policy: { max_total_bytes: 50 * 1024 * 1024 * 1024, max_age_days: 30 },
    });
    render();
    await settle();
    expect(store.settings.retention).toEqual({
      sizeEnabled: true,
      sizeGb: "50",
      ageEnabled: true,
      ageDays: "30",
    });
  });

  it("shows nothing about deletions until a limit is set", async () => {
    const el = render();
    await settle();
    expect(el.querySelector(".callout-warn")).toBeNull();
  });
});

describe("about", () => {
  it("shows the built version rather than a hard-coded one", async () => {
    const el = render();
    await settle();
    expect(el.textContent).toContain(__APP_VERSION__);
  });
});
