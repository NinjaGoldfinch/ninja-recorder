import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createAutosave, type SaveStatus } from "./autosave";

/** A save whose completion the test decides. */
function controlledSave() {
  const calls: { value: string; resolve: () => void; reject: (e: unknown) => void }[] = [];
  const save = vi.fn(
    (value: string) =>
      new Promise<void>((resolve, reject) => {
        calls.push({ value, resolve, reject });
      }),
  );
  return { save, calls };
}

async function settle() {
  for (let i = 0; i < 10; i++) await Promise.resolve();
}

describe("autosave", () => {
  let statuses: SaveStatus[];
  const onStatus = (s: SaveStatus) => statuses.push(s);

  beforeEach(() => {
    vi.useFakeTimers();
    statuses = [];
  });
  afterEach(() => vi.useRealTimers());

  it("waits for a quiet period and then saves the last value once", async () => {
    const save = vi.fn(async () => {});
    const autosave = createAutosave({ save, delayMs: 500, onStatus });

    autosave.change("a");
    vi.advanceTimersByTime(300);
    autosave.change("ab");
    vi.advanceTimersByTime(300);
    expect(save).not.toHaveBeenCalled();

    vi.advanceTimersByTime(200);
    await settle();
    expect(save).toHaveBeenCalledTimes(1);
    expect(save).toHaveBeenCalledWith("ab");
    expect(statuses[statuses.length - 1]).toBe("saved");
  });

  it("never runs two saves at once, and saves the newest value after the one in flight", async () => {
    const { save, calls } = controlledSave();
    const autosave = createAutosave({ save, delayMs: 100, onStatus });

    autosave.change("first");
    vi.advanceTimersByTime(100);
    await settle();
    expect(calls).toHaveLength(1);

    autosave.change("second");
    autosave.change("third");
    vi.advanceTimersByTime(1000);
    await settle();
    expect(calls, "nothing starts while a save is running").toHaveLength(1);

    calls[0].resolve();
    await settle();
    expect(calls.map((c) => c.value)).toEqual(["first", "third"]);
    calls[1].resolve();
    await settle();
    expect(statuses[statuses.length - 1]).toBe("saved");
  });

  it("reports a failed save as an error and keeps the value for the next attempt", async () => {
    const { save, calls } = controlledSave();
    const autosave = createAutosave({ save, delayMs: 100, onStatus });

    autosave.change("x");
    vi.advanceTimersByTime(100);
    await settle();
    calls[0].reject(new Error("not connected"));
    await settle();
    expect(statuses[statuses.length - 1]).toBe("error");

    const flushed = autosave.flush();
    await settle();
    expect(calls.map((c) => c.value)).toEqual(["x", "x"]);
    calls[1].resolve();
    await flushed;
    expect(statuses[statuses.length - 1]).toBe("saved");
  });

  it("flush saves immediately, and does nothing when everything is saved", async () => {
    const save = vi.fn(async () => {});
    const autosave = createAutosave({ save, delayMs: 10_000, onStatus });

    await autosave.flush();
    expect(save).not.toHaveBeenCalled();

    autosave.change("now");
    await autosave.flush();
    expect(save).toHaveBeenCalledWith("now");
    vi.advanceTimersByTime(10_000);
    await settle();
    expect(save, "the debounced save is not repeated").toHaveBeenCalledTimes(1);
  });

  it("cancel drops a pending save", async () => {
    const save = vi.fn(async () => {});
    const autosave = createAutosave({ save, delayMs: 100, onStatus });
    autosave.change("gone");
    autosave.cancel();
    vi.advanceTimersByTime(1000);
    await settle();
    expect(save).not.toHaveBeenCalled();
  });

  it("marks a change unsaved straight away", () => {
    const autosave = createAutosave({ save: async () => {}, delayMs: 100, onStatus });
    autosave.change("a");
    expect(statuses).toEqual(["unsaved"]);
  });
});
