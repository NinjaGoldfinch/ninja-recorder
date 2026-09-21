import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

/**
 * The portal's shared poll.
 *
 * **One `dev_health` round trip, fanned out.** `main.ts` ran the timer and
 * pushed into whichever panel was showing; a page where five panels each
 * started their own would throw the single round trip away.
 */

const tryCall = vi.hoisted(() => vi.fn());
vi.mock("../../dev/ipc", () => ({ tryCall, call: vi.fn(), POLLED_COMMANDS: new Set() }));

let store: typeof import("./dev.svelte");

const ok = <T>(value: T) => ({ ok: true as const, value });
const err = (error: string) => ({ ok: false as const, error });

beforeEach(async () => {
  vi.resetModules();
  vi.useFakeTimers();
  tryCall.mockReset();
  store = await import("./dev.svelte");
});

afterEach(() => {
  store.stopPolling();
  vi.useRealTimers();
});

describe("the environment", () => {
  it("is read once at boot", async () => {
    tryCall.mockResolvedValue(ok({ db_path: "C:/db.sqlite", os: "windows" }));
    await store.loadEnv();
    expect(store.dev.env?.os).toBe("windows");
    expect(store.dev.envError).toBeNull();
  });

  it("records why it failed, which is the one failure worth spelling out", async () => {
    // Reaching dev.html in a build without the `devtools` feature otherwise
    // makes every panel show an opaque "command not found".
    tryCall.mockResolvedValue(err("command dev_env_info not found"));
    await store.loadEnv();
    expect(store.dev.env).toBeNull();
    expect(store.dev.envError).toContain("dev_env_info");
  });
});

describe("the health poll", () => {
  it("asks immediately when switched on, then every second", async () => {
    tryCall.mockResolvedValue(ok({ is_recording: false }));
    store.setLive(true);
    await vi.advanceTimersByTimeAsync(0);
    expect(tryCall).toHaveBeenCalledTimes(1);

    await vi.advanceTimersByTimeAsync(1000);
    expect(tryCall).toHaveBeenCalledTimes(2);
    await vi.advanceTimersByTimeAsync(1000);
    expect(tryCall).toHaveBeenCalledTimes(3);
  });

  it("stops when switched off, and asks nothing more", async () => {
    tryCall.mockResolvedValue(ok({ is_recording: false }));
    store.setLive(true);
    await vi.advanceTimersByTimeAsync(0);
    const after = tryCall.mock.calls.length;

    store.setLive(false);
    await vi.advanceTimersByTimeAsync(5000);
    expect(tryCall).toHaveBeenCalledTimes(after);
    expect(store.dev.live).toBe(false);
  });

  it("does not stack timers when switched on twice", async () => {
    // `setLive` clears before it sets, so a double toggle cannot leave two
    // intervals polling the backend.
    tryCall.mockResolvedValue(ok({ is_recording: false }));
    store.setLive(true);
    store.setLive(true);
    await vi.advanceTimersByTimeAsync(0);
    tryCall.mockClear();

    await vi.advanceTimersByTimeAsync(1000);
    expect(tryCall).toHaveBeenCalledTimes(1);
  });

  it("publishes what it got", async () => {
    tryCall.mockResolvedValue(ok({ is_recording: true, free_bytes: 42 }));
    store.setLive(true);
    await vi.advanceTimersByTimeAsync(0);
    expect(store.dev.health?.is_recording).toBe(true);
    expect(store.dev.healthError).toBeNull();
  });

  it("reports a failure and clears the stale reading", async () => {
    // A pill showing last second's state while the backend is unreachable is
    // worse than one that says so.
    tryCall.mockResolvedValueOnce(ok({ is_recording: true }));
    store.setLive(true);
    await vi.advanceTimersByTimeAsync(0);

    tryCall.mockResolvedValue(err("not connected"));
    await vi.advanceTimersByTimeAsync(1000);

    expect(store.dev.health).toBeNull();
    expect(store.dev.healthError).toBe("not connected");
  });

  it("recovers when the backend comes back", async () => {
    tryCall.mockResolvedValue(err("not connected"));
    store.setLive(true);
    await vi.advanceTimersByTimeAsync(0);

    tryCall.mockResolvedValue(ok({ is_recording: false }));
    await vi.advanceTimersByTimeAsync(1000);
    expect(store.dev.healthError).toBeNull();
    expect(store.dev.health).not.toBeNull();
  });
});

describe("splitDbPath", () => {
  it("splits at the last separator so only the directory can shrink", () => {
    expect(store.splitDbPath("C:\\Users\\me\\db.sqlite")).toEqual({
      dir: "C:\\Users\\me",
      file: "\\db.sqlite",
    });
    expect(store.splitDbPath("/home/me/db.sqlite")).toEqual({
      dir: "/home/me",
      file: "/db.sqlite",
    });
  });

  it("puts a bare filename entirely in the half that does not shrink", () => {
    expect(store.splitDbPath("db.sqlite")).toEqual({ dir: "", file: "db.sqlite" });
  });
});
