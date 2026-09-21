/**
 * When the window is allowed to talk to the recorder. WS3 task 3.8.
 *
 * The bug this pins is a race, which is the kind that survives review and then
 * greets someone on their first launch: the window paints before the handshake
 * finishes, so anything fetched on load failed with `not connected to the
 * recorder` and was reported as an error. `whenDaemonReachable` is the answer,
 * and what makes it correct is timing rather than logic, so it is worth a test
 * that can only fail by regressing the timing.
 *
 * The reconnect half is tested for a different reason: it never worked. The
 * strip cleared itself when the daemon came back and nothing re-read anything,
 * so a window that lost its recorder kept showing what it had when the
 * connection died. That is invisible in review and obvious in use.
 */

import { beforeEach, describe, expect, it, vi } from "vitest";
import type { DaemonHealth } from "../transport/pipe";

/** What the one-off `daemonHealth()` call answers with. */
let firstAnswer: Promise<DaemonHealth>;
/** The subscription's handler, so a test can push health changes in. */
let push: ((health: DaemonHealth) => void) | undefined;

/** Never settles: the window has asked and nothing has answered yet. */
function unanswered(): Promise<DaemonHealth> {
  return new Promise<DaemonHealth>(() => {});
}

/** Lets every already-resolved promise run its continuations. */
async function settle() {
  await Promise.resolve();
  await Promise.resolve();
}

async function load(inTauri = true) {
  vi.resetModules();
  push = undefined;
  vi.doMock("../transport/invoke", () => ({ IN_TAURI: inTauri }));
  vi.doMock("../transport/pipe", () => ({
    daemonHealth: () => firstAnswer,
    subscribe: (handlers: { onHealth?: (health: DaemonHealth) => void }) => {
      push = handlers.onHealth;
      return () => {};
    },
  }));
  const daemon = await import("./daemon.svelte");
  daemon.initDaemonStatus();
  return daemon;
}

describe("whenDaemonReachable", () => {
  beforeEach(() => {
    firstAnswer = unanswered();
  });

  /** The whole point: "not known yet" must not be mistaken for "connected". */
  it("does not run before anything has said the daemon is there", async () => {
    const { whenDaemonReachable } = await load();
    const run = vi.fn();

    whenDaemonReachable(run);
    await settle();

    expect(run).not.toHaveBeenCalled();
  });

  it("runs once the first handshake completes", async () => {
    firstAnswer = Promise.resolve({ state: "connected" });
    const { whenDaemonReachable } = await load();
    const run = vi.fn();

    whenDaemonReachable(run);
    await settle();

    expect(run).toHaveBeenCalledTimes(1);
  });

  /** So a caller never has to ask which of the two cases it is in. */
  it("runs immediately when registered against a live connection", async () => {
    firstAnswer = Promise.resolve({ state: "connected" });
    const { whenDaemonReachable } = await load();
    await settle();

    const run = vi.fn();
    whenDaemonReachable(run);

    expect(run).toHaveBeenCalledTimes(1);
  });

  /** The half that never worked: the daemon dies, comes back, and is re-read. */
  it("runs again when the daemon comes back", async () => {
    firstAnswer = Promise.resolve({ state: "connected" });
    const { whenDaemonReachable } = await load();
    const run = vi.fn();
    whenDaemonReachable(run);
    await settle();

    push?.({ state: "reconnecting" });
    push?.({ state: "connected" });

    expect(run).toHaveBeenCalledTimes(2);
  });

  /**
   * A reconnect is a transition, not a report. `ui::client` is free to say
   * "connected" as often as it likes, and re-fetching the library on each one
   * would put a burst of RPCs behind every health message.
   */
  it("does not run again when a connection that never dropped reports itself", async () => {
    firstAnswer = Promise.resolve({ state: "connected" });
    const { whenDaemonReachable } = await load();
    const run = vi.fn();
    whenDaemonReachable(run);
    await settle();

    push?.({ state: "connected" });
    push?.({ state: "connected" });

    expect(run).toHaveBeenCalledTimes(1);
  });

  /**
   * There is no pipe in a browser and the mock transport answers everything,
   * so waiting for a handshake would mean waiting forever and rendering
   * nothing — the fixtures exist precisely so the frontend can be worked on
   * without a daemon.
   */
  it("runs immediately outside Tauri, where there is no daemon to wait for", async () => {
    const { whenDaemonReachable } = await load(false);
    const run = vi.fn();

    whenDaemonReachable(run);

    expect(run).toHaveBeenCalledTimes(1);
  });
});

describe("what the strip says", () => {
  // WS4.6 moved the element to `DaemonStrip.svelte`; these assert the wording
  // rather than the DOM, which is where the wording lives now.
  beforeEach(() => {
    firstAnswer = unanswered();
  });

  it("says nothing until something has answered", async () => {
    // "Not known yet" is not "not connected", and the strip must not claim
    // the recorder is missing while the handshake is still in flight.
    const { daemon } = await load();
    expect(daemon.strip).toBeNull();
  });

  it("says the recorder is missing, and stops saying it when it is back", async () => {
    const { daemon } = await load();

    push?.({ state: "reconnecting" });
    expect(daemon.strip?.kind).toBe("warn");
    expect(daemon.strip?.text).toContain("The recorder is not running");

    push?.({ state: "connected" });
    expect(daemon.strip).toBeNull();
  });

  /** Terminal, so it says restart rather than implying waiting will help. */
  it("tells a skewed build to restart, and names both protocols", async () => {
    const { daemon } = await load();

    push?.({ state: "skewed", ours: 3, theirs: 4 });

    expect(daemon.strip?.kind).toBe("error");
    expect(daemon.strip?.text).toContain("3");
    expect(daemon.strip?.text).toContain("4");
    expect(daemon.strip?.text).toContain("Restart");
  });

  it("offers no button for a skew, deliberately", async () => {
    // The two builds cannot agree, and the one thing the UI must not do is
    // tell the daemon to quit: it may be recording. The strip is text and a
    // kind, and has nowhere to put an action.
    const { daemon } = await load();
    push?.({ state: "skewed", ours: 3, theirs: 4 });
    expect(Object.keys(daemon.strip ?? {})).toEqual(["kind", "text"]);
  });
});
