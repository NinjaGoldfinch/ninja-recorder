import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { GameState } from "../../types";

/**
 * The poll behind the app bar's two pills.
 *
 * Nothing pushes the header's live state, so it comes from a poll, and the
 * poll's cadence is most of what this module is. A `setTimeout` chain rather
 * than `setInterval`, because `lcu_status` reads a lockfile and makes two
 * HTTPS round trips to the client: a slow tick under `setInterval` would stack
 * calls on top of each other, and chaining makes overlap structurally
 * impossible rather than guarded against.
 */

const call = vi.hoisted(() => vi.fn());
vi.mock("../../bridge", () => ({ call, hasDevCommands: vi.fn(), assetUrl: (p: string) => p }));
const refreshLibrary = vi.hoisted(() => vi.fn());
const refreshDiskUsage = vi.hoisted(() => vi.fn());
vi.mock("./library.svelte", () => ({ refreshLibrary, refreshDiskUsage }));
const refreshUpdateStatus = vi.hoisted(() => vi.fn());
vi.mock("./update.svelte", () => ({ refreshUpdateStatus }));
const refreshCaptureBackend = vi.hoisted(() => vi.fn());
vi.mock("./settings.svelte", () => ({ refreshCaptureBackend }));

let status: typeof import("./status.svelte");
let about: typeof import("./about.svelte");

const supervisor = (state: GameState, over: Record<string, unknown> = {}) => ({
  state,
  recording_elapsed_s: null,
  last_finalized: null,
  ...over,
});

const lcu = (over: Record<string, unknown> = {}) => ({
  connected: true,
  summoner: "Ninja",
  phase: "Lobby",
  error: null,
  ...over,
});

/** Answers whichever command is asked, so the chain can be driven by hand. */
function backend(supervisorStatus: unknown, lcuStatus: unknown = lcu()) {
  call.mockImplementation((name: string) =>
    Promise.resolve(name === "lcu_status" ? lcuStatus : supervisorStatus),
  );
}

beforeEach(async () => {
  vi.resetModules();
  vi.useFakeTimers();
  call.mockReset();
  refreshLibrary.mockReset();
  refreshDiskUsage.mockReset();
  refreshUpdateStatus.mockReset();
  refreshCaptureBackend.mockReset();

  about = await import("./about.svelte");
  status = await import("./status.svelte");
});

afterEach(() => {
  status.stopStatusPolling();
  vi.useRealTimers();
});

/** Lets the in-flight promises settle without advancing the clock. */
const settle = async () => {
  for (let i = 0; i < 6; i++) await Promise.resolve();
};

describe("the first tick", () => {
  it("fills in both pills", async () => {
    backend(supervisor("Idle"));
    status.initStatus();
    await settle();

    expect(about.about.lcuPill.copy).toBe("Ninja");
    expect(about.about.gamePill.copy).toBe("Idle");
    expect(about.about.gameState).toBe("Idle");
  });

  it("asks the expensive command once, alongside the cheap one", async () => {
    // `lcu_status` reads a lockfile and makes two round trips;
    // `game_state_status` is a mutex read with no I/O.
    backend(supervisor("Idle"));
    status.initStatus();
    await settle();

    const names = call.mock.calls.map((c) => c[0]);
    expect(names.filter((n) => n === "game_state_status")).toHaveLength(1);
    expect(names.filter((n) => n === "lcu_status")).toHaveLength(1);
  });
});

describe("the cadence", () => {
  it("polls faster while recording than while idle", async () => {
    backend(supervisor("Recording"));
    status.initStatus();
    await settle();
    const after = call.mock.calls.length;

    // 1500ms while recording.
    vi.advanceTimersByTime(1500);
    await settle();
    expect(call.mock.calls.length).toBeGreaterThan(after);
  });

  it("does not poll again before the interval is up", async () => {
    backend(supervisor("Idle"));
    status.initStatus();
    await settle();
    const after = call.mock.calls.length;

    // Idle is 5000ms.
    vi.advanceTimersByTime(4000);
    await settle();
    expect(call.mock.calls.length).toBe(after);
  });

  it("stops entirely when asked", async () => {
    backend(supervisor("Idle"));
    status.initStatus();
    await settle();
    status.stopStatusPolling();
    const after = call.mock.calls.length;

    vi.advanceTimersByTime(60_000);
    await settle();
    expect(call.mock.calls.length).toBe(after);
  });
});

describe("what it refreshes, and when", () => {
  it("re-reads the update status on a state change and not otherwise", async () => {
    // Whether an offered update can be installed depends on this exact value,
    // and the check that computed it last runs every six hours. Every tick
    // would be waste; never would leave Install enabled through a whole game.
    backend(supervisor("Idle"));
    status.initStatus();
    await settle();
    expect(refreshUpdateStatus).toHaveBeenCalledOnce();

    refreshUpdateStatus.mockClear();
    vi.advanceTimersByTime(5000);
    await settle();
    expect(refreshUpdateStatus).not.toHaveBeenCalled();

    backend(supervisor("Recording"));
    vi.advanceTimersByTime(5000);
    await settle();
    expect(refreshUpdateStatus).toHaveBeenCalledOnce();
  });

  // The own backend learns its encoder when the client opens, so the
  // software-encoding notice in Settings can only appear after an edge.
  it("re-reads the capture backend on a state change and not otherwise", async () => {
    backend(supervisor("Idle"));
    status.initStatus();
    await settle();
    expect(refreshCaptureBackend).toHaveBeenCalledOnce();

    refreshCaptureBackend.mockClear();
    vi.advanceTimersByTime(5000);
    await settle();
    expect(refreshCaptureBackend).not.toHaveBeenCalled();

    backend(supervisor("ClientRunning"));
    vi.advanceTimersByTime(5000);
    await settle();
    expect(refreshCaptureBackend).toHaveBeenCalledOnce();
  });

  it("refreshes the library when a game stops finalizing", async () => {
    // A finished game should appear on its own, derived from the edges already
    // in the payload rather than by polling `list_recordings`, which would
    // rebuild the grid every couple of seconds and fight scroll and focus.
    backend(supervisor("Finalizing"));
    status.initStatus();
    await settle();
    refreshLibrary.mockClear();

    backend(supervisor("Idle"));
    vi.advanceTimersByTime(1500);
    await settle();
    expect(refreshLibrary).toHaveBeenCalled();
  });

  it("refreshes when a new recording appears", async () => {
    backend(supervisor("Idle"));
    status.initStatus();
    await settle();
    refreshLibrary.mockClear();

    backend(
      supervisor("Idle", { last_finalized: { path: "C:/a.mp4", markers: [], recording_id: 1 } }),
    );
    vi.advanceTimersByTime(5000);
    await settle();
    expect(refreshLibrary).toHaveBeenCalled();
  });

  it("does not refresh on an unchanged quiet tick", async () => {
    backend(supervisor("Idle"));
    status.initStatus();
    await settle();
    refreshLibrary.mockClear();

    vi.advanceTimersByTime(5000);
    await settle();
    expect(refreshLibrary).not.toHaveBeenCalled();
  });
});

describe("when the backend cannot answer", () => {
  it("says the status is unavailable rather than leaving the last one up", async () => {
    call.mockRejectedValue(new Error("not connected"));
    status.initStatus();
    await settle();

    expect(about.about.gamePill.state).toBe("error");
    expect(about.about.gamePill.copy).toBe("Status unavailable");
    expect(about.about.gameState).toContain("Failed to read");
  });

  it("keeps polling afterwards", async () => {
    // A daemon that went away comes back, and a chain that stopped on the
    // first failure would never notice.
    call.mockRejectedValue(new Error("not connected"));
    status.initStatus();
    await settle();
    const after = call.mock.calls.length;

    vi.advanceTimersByTime(5000);
    await settle();
    expect(call.mock.calls.length).toBeGreaterThan(after);
  });
});
