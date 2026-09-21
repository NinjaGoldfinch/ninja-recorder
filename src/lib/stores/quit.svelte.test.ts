import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

/**
 * What "Quit" means now that the recorder is a different process.
 *
 * The close button's `quit` option used to end the window and nothing else:
 * the daemon kept recording, kept its tray icon, and the app was visibly still
 * running. The setting said one thing and did another, which is worse than not
 * offering it.
 *
 * **Two calls, in this order.** `quit_recorder` stops the recorder, finalizing
 * whatever is in flight on the way out; `exit_ui` ends the window. The window
 * always goes; the recorder only goes if the person said so.
 */

const call = vi.hoisted(() => vi.fn());
vi.mock("../../bridge", () => ({ call, hasDevCommands: vi.fn(), assetUrl: (p: string) => p }));
const toast = vi.hoisted(() => vi.fn());
vi.mock("./toast.svelte", () => ({ toast }));

let quit: typeof import("./quit.svelte");

beforeEach(async () => {
  vi.resetModules();
  call.mockReset();
  toast.mockReset();
  quit = await import("./quit.svelte");
});

afterEach(() => {
  quit.setQuitAsker(null);
});

describe("quitting with nothing recording", () => {
  it("stops the recorder and then the window, in that order", async () => {
    call.mockResolvedValueOnce({ outcome: "shuttingDown" }).mockResolvedValueOnce(undefined);
    await quit.quitEverything();

    expect(call.mock.calls.map((c) => c[0])).toEqual(["quit_recorder", "exit_ui"]);
    expect(call.mock.calls[0][1]).toEqual({ force: false });
  });

  it("never asks when the daemon did not refuse", async () => {
    const ask = vi.fn().mockResolvedValue(true);
    quit.setQuitAsker(ask);
    call.mockResolvedValueOnce({ outcome: "shuttingDown" }).mockResolvedValueOnce(undefined);

    await quit.quitEverything();
    expect(ask).not.toHaveBeenCalled();
  });
});

describe("quitting mid-recording", () => {
  it("asks, and stops when the answer is no", async () => {
    // The daemon refuses rather than putting up its own modal: a `MessageBoxW`
    // with no owner window can appear *behind* the window the person just
    // clicked.
    const ask = vi.fn().mockResolvedValue(false);
    quit.setQuitAsker(ask);
    call.mockResolvedValueOnce({ outcome: "recordingInFlight" });

    await quit.quitEverything();

    expect(ask).toHaveBeenCalledOnce();
    expect(call.mock.calls.map((c) => c[0])).toEqual(["quit_recorder"]);
  });

  it("asks the daemon again with the answer rather than trusting the first reply", async () => {
    // A game can end between the question and the click, and the daemon
    // deciding twice is cheaper than this side deciding once.
    const ask = vi.fn().mockResolvedValue(true);
    quit.setQuitAsker(ask);
    call
      .mockResolvedValueOnce({ outcome: "recordingInFlight" })
      .mockResolvedValueOnce({ outcome: "shuttingDown" })
      .mockResolvedValueOnce(undefined);

    await quit.quitEverything();

    expect(call.mock.calls.map((c) => c[0])).toEqual(["quit_recorder", "quit_recorder", "exit_ui"]);
    expect(call.mock.calls[1][1]).toEqual({ force: true });
  });

  it("refuses to quit when there is nothing to ask with", async () => {
    // No dialog means no way to ask, and quitting anyway would end a recording
    // nobody agreed to lose.
    call.mockResolvedValueOnce({ outcome: "recordingInFlight" });
    await quit.quitEverything();
    expect(call.mock.calls.map((c) => c[0])).toEqual(["quit_recorder"]);
  });
});

describe("when it does not work", () => {
  it("says so rather than leaving a window that appears to ignore its own close button", async () => {
    call.mockResolvedValueOnce({ outcome: "somethingElse" });
    await quit.quitEverything();

    expect(toast).toHaveBeenCalledWith(
      "The recorder would not stop, so nothing was quit.",
      "error",
    );
    expect(call.mock.calls.map((c) => c[0])).toEqual(["quit_recorder"]);
  });

  it("reports a failure with its reason", async () => {
    call.mockRejectedValueOnce(new Error("not connected"));
    await quit.quitEverything();
    expect(toast).toHaveBeenCalledWith(expect.stringContaining("not connected"), "error");
  });
});
