/**
 * The daemon transport's routing. WS3 task 3.4.
 *
 * Two claims worth pinning, both of which are wiring rather than logic and so
 * fail silently when they break.
 *
 * The first is that a command reaches the daemon at all: everything goes
 * through `rpc_call` with its name and arguments intact, because the daemon
 * routes by name and a transport that renamed or reshaped anything would turn
 * every view into a runtime error.
 *
 * The second is that the handful of shell commands do *not*. Sending
 * `open_recordings_folder` to the daemon would open an Explorer window from a
 * background process, which lands behind the foreground app. That is the kind
 * of bug nobody writes a test for after it ships, because by then it reads as
 * a platform quirk.
 */

import { beforeEach, describe, expect, it, vi } from "vitest";

// Typed as the two functions they stand in for, so `mock.calls` indexes and
// `tsc` agrees. `vi.fn()` on its own infers `[]` for its arguments, which makes
// every `calls[0][0]` an error.
const invoke = vi.fn<(command: string, args?: unknown) => Promise<unknown>>();
const listen = vi.fn<(channel: string, handler: (e: unknown) => void) => Promise<() => void>>();

vi.mock("@tauri-apps/api/core", () => ({ invoke: (c: string, a?: unknown) => invoke(c, a) }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: (c: string, h: (e: unknown) => void) => listen(c, h),
}));

const { pipeTransport, subscribe, currentSnapshot } = await import("./pipe");

describe("the pipe transport", () => {
  beforeEach(() => {
    invoke.mockReset();
    invoke.mockResolvedValue(null);
    listen.mockClear();
  });

  it("forwards a command to the daemon by name, arguments untouched", async () => {
    await pipeTransport.invoke("get_recording_markers", { recordingId: 7 });

    expect(invoke).toHaveBeenCalledWith("rpc_call", {
      command: "get_recording_markers",
      args: { recordingId: 7 },
    });
  });

  it("sends an empty object rather than undefined for a command with no arguments", async () => {
    await pipeTransport.invoke("is_recording", undefined as never);

    expect(invoke).toHaveBeenCalledWith("rpc_call", { command: "is_recording", args: {} });
  });

  /** The daemon has no window and no foreground, so these stay here. */
  it("keeps the shell commands in this process", async () => {
    await pipeTransport.invoke("open_recordings_folder", {});

    expect(invoke).toHaveBeenCalledWith("open_recordings_folder", {});
  });

  /**
   * The portal decides whether it exists by watching this call reject in a
   * shipped build. Routed through `rpc_call` it would reject in *every* build,
   * with "unknown command", permanently hiding the button.
   */
  it("keeps dev_registered_commands direct", async () => {
    await pipeTransport.invoke("dev_registered_commands", {});

    expect(invoke).toHaveBeenCalledWith("dev_registered_commands", {});
  });

  it("asks the UI process for the snapshot it already has", async () => {
    await currentSnapshot();

    // Only the name is asserted: the stand-in above always forwards a second
    // argument, and whether it is `undefined` is the mock's business rather
    // than the transport's.
    expect(invoke.mock.calls[0][0]).toBe("rpc_subscribe");
  });
});

describe("subscribing", () => {
  beforeEach(() => {
    listen.mockClear();
    listen.mockImplementation(async () => () => {});
  });

  /**
   * Three channels, not one. A snapshot *replaces* the frontend's world where
   * an event *updates* it, and the connection's health is neither: it is
   * something that happened to the wire rather than in the recorder.
   */
  it("listens on the three channels the daemon pushes on", () => {
    subscribe({ onSnapshot: () => {}, onEvent: () => {}, onHealth: () => {} });

    expect(listen.mock.calls.map((c) => c[0])).toEqual(["snapshot", "event", "daemon-health"]);
  });

  it("listens only for what the caller asked for", () => {
    subscribe({ onEvent: () => {} });

    expect(listen.mock.calls.map((c) => c[0])).toEqual(["event"]);
  });

  it("hands back a teardown that stops every listener", async () => {
    const off = vi.fn();
    listen.mockImplementation(async () => off);

    const stop = subscribe({ onSnapshot: () => {}, onEvent: () => {} });
    stop();
    // The unlisten handles resolve asynchronously; teardown is deliberately
    // synchronous so a view unmounting can call it.
    await vi.waitFor(() => expect(off).toHaveBeenCalledTimes(2));
  });
});
