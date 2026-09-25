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
// A release build: nothing in this view asks, and no test may depend on the
// dev portal being there.
const hasDevCommands = vi.hoisted(() => vi.fn());
vi.mock("../../../bridge", () => ({
  call,
  hasDevCommands,
  assetUrl: (p: string) => p,
}));
// `whenDaemonReachable` runs its callback once the handshake lands. The view
// uses it to gate five RPCs; here it fires immediately so those paths run.
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

const NOT_BUILT = "the own capture backend is not in this build yet";

/** Both backends buildable: a release build on Windows 11. */
const BOTH_BUILT = [
  { backend: "libobs", unavailable: null },
  { backend: "own", unavailable: null },
];

/** The own backend below its OS floor, as on Windows 10. */
const OWN_UNBUILT = [
  { backend: "libobs", unavailable: null },
  { backend: "own", unavailable: NOT_BUILT },
];

/**
 * What the daemon reports for someone who saved libobs, on a build where both
 * backends can be built.
 */
function backendStatus(over: Record<string, unknown> = {}) {
  return {
    configured: "libobs",
    automatic: false,
    active: "libobs (idle)",
    software_encoding: false,
    options: BOTH_BUILT,
    ...over,
  };
}

/** Answers each RPC the view makes on mount with something plausible. */
function stubBackend(over: Record<string, unknown> = {}) {
  const answers: Record<string, unknown> = {
    get_autostart: { enabled: false, supported: true },
    get_retention_policy: { max_total_bytes: null, max_age_days: null },
    get_recordings_dir: "C:/vods",
    list_audio_inputs: [],
    get_audio_preset: { preset: "game" },
    get_update_status: { kind: "upToDate" },
    get_capture_backend: backendStatus(),
    ...over,
  };
  call.mockImplementation((name: string) =>
    name in answers ? Promise.resolve(answers[name]) : Promise.resolve(undefined),
  );
}

beforeEach(async () => {
  vi.resetModules();
  call.mockReset();
  hasDevCommands.mockReset();
  hasDevCommands.mockResolvedValue(false);
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

describe("the capture backend", () => {
  // Every build since #243: a release build has no dev commands, and the row
  // is there regardless.
  it("always renders, in a release build too", async () => {
    const el = render();
    await settle();
    expect(el.querySelector('[aria-label="Capture backend"]')).not.toBeNull();
    expect(el.textContent).toContain("Advanced");
    expect(hasDevCommands).not.toHaveBeenCalled();
  });

  it("explains each backend in a line", async () => {
    const el = render();
    await settle();
    expect(el.textContent).toContain("Own: the default");
    expect(el.textContent).toContain("libobs: the recorder earlier versions used");
  });

  const choice = (el: HTMLElement, label: string) =>
    [...el.querySelectorAll<HTMLButtonElement>('[aria-label="Capture backend"] button')].find(
      (b) => b.textContent?.trim() === label,
    );

  it("renders every backend and shows the saved one as chosen", async () => {
    const el = render();
    await settle();
    expect(choice(el, "libobs")?.getAttribute("aria-checked")).toBe("true");
    expect(choice(el, "Own")?.getAttribute("aria-checked")).toBe("false");
    expect(el.textContent).toContain("libobs (idle)");
    expect(el.textContent).toContain("Applies from the next recording");
  });

  // Listed rather than left out, so the row can say why it cannot be picked.
  it("disables a backend this build cannot construct, and says why", async () => {
    stubBackend({ get_capture_backend: backendStatus({ options: OWN_UNBUILT }) });
    const el = render();
    await settle();
    const own = choice(el, "Own");
    expect(own?.disabled).toBe(true);
    expect(own?.title).toBe(NOT_BUILT);
    expect(choice(el, "libobs")?.disabled).toBe(false);
    expect(el.textContent).toContain(`Own isn't available: ${NOT_BUILT}.`);
  });

  it("asks the daemon to switch, and shows what it reports back", async () => {
    const answers = call.getMockImplementation() as (n: string, a?: unknown) => unknown;
    call.mockImplementation((name: string, args?: unknown) =>
      name === "set_capture_backend"
        ? Promise.resolve(
            backendStatus({ configured: "own", active: "own (idle)", options: BOTH_BUILT }),
          )
        : answers(name, args),
    );
    const el = render();
    await settle();

    choice(el, "Own")?.click();
    await settle();

    expect(call).toHaveBeenCalledWith("set_capture_backend", { backend: "own" });
    expect(choice(el, "Own")?.getAttribute("aria-checked")).toBe("true");
    expect(el.textContent).toContain("own (idle)");
  });

  it("does not ask again for the backend that is already chosen", async () => {
    const el = render();
    await settle();
    choice(el, "libobs")?.click();
    await settle();
    expect(call).not.toHaveBeenCalledWith("set_capture_backend", expect.anything());
  });

  // Mid-game the daemon refuses, and the control must not claim a switch that
  // did not happen.
  it("keeps the saved backend when the daemon refuses", async () => {
    const answers = call.getMockImplementation() as (n: string, a?: unknown) => unknown;
    call.mockImplementation((name: string, args?: unknown) =>
      name === "set_capture_backend"
        ? Promise.reject(
            new Error("The capture backend can't be changed while a recording is in progress"),
          )
        : answers(name, args),
    );
    const toasts = await import("../../stores/toast.svelte");
    const el = render();
    await settle();

    choice(el, "Own")?.click();
    await settle();

    expect(choice(el, "libobs")?.getAttribute("aria-checked")).toBe("true");
    expect(toasts.toastState.message).toContain("recording is in progress");
  });

  it("says nothing about software encoding while the encoder is hardware", async () => {
    const el = render();
    await settle();
    expect(el.textContent).not.toContain("encoding video in software");
  });

  // DEVELOPMENT.md §2.4: the own backend's software fallback is never silent.
  it("shows a notice while the own backend encodes in software", async () => {
    stubBackend({
      get_capture_backend: backendStatus({
        configured: "own",
        active: "own (software encoding: H264 Encoder MFT, because no hardware GPU was found)",
        software_encoding: true,
      }),
    });
    const el = render();
    await settle();
    const notice = [...el.querySelectorAll(".callout-warn")].find((n) =>
      n.textContent?.includes("encoding video in software"),
    );
    expect(notice?.textContent).toContain("more CPU");
  });

  // Windows 10 with nothing saved: libobs records, and the row says why.
  it("says which backend is automatic, and why, when nothing is saved", async () => {
    stubBackend({
      get_capture_backend: backendStatus({
        automatic: true,
        active: "libobs (idle)",
        options: OWN_UNBUILT,
      }),
    });
    const el = render();
    await settle();
    expect(el.textContent).toContain(`Automatic: libobs, because ${NOT_BUILT}.`);
    expect(el.querySelector(".callout-warn")).toBeNull();
  });

  // Only a click writes the row, and clicking the automatic pick is one.
  it("saves the automatic pick when it is clicked", async () => {
    stubBackend({
      get_capture_backend: backendStatus({ configured: "own", automatic: true }),
    });
    const el = render();
    await settle();
    expect(call).not.toHaveBeenCalledWith("set_capture_backend", expect.anything());
    choice(el, "Own")?.click();
    await settle();
    expect(call).toHaveBeenCalledWith("set_capture_backend", { backend: "own" });
  });

  // A saved own on Windows 10: refused, never moved to libobs.
  it("warns that nothing will be recorded when the saved backend cannot be built", async () => {
    stubBackend({
      get_capture_backend: backendStatus({
        configured: "own",
        active: `unavailable (${NOT_BUILT})`,
        options: OWN_UNBUILT,
      }),
    });
    const el = render();
    await settle();
    expect(el.querySelector(".callout-warn")?.textContent).toContain("Nothing will be recorded");
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

  // The installer ships the file under exactly this name
  // (tauri.windows.conf.json), so the panel must name the same one.
  it("says where the third-party notices are", async () => {
    const el = render();
    await settle();
    expect(el.textContent).toContain("THIRD_PARTY_NOTICES.txt");
  });
});
