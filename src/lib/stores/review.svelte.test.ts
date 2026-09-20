import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

/**
 * The review session: which recording is open and what its timeline is.
 *
 * What this store deliberately does *not* hold is the player. `currentTime`,
 * `paused`, `volume` and the rest live on the elements that own them, because
 * a `<video>`'s position is not state anything should be diffing.
 */

const call = vi.hoisted(() => vi.fn());
vi.mock("../../bridge", () => ({ call, hasDevCommands: vi.fn(), assetUrl: (p: string) => p }));

let store: typeof import("./review.svelte");

const row = (id: number) => ({ id, path: `C:/vods/${id}.mp4` }) as never;
const sample = (video_time_s: number, game_time_s: number) =>
  ({ video_time_s, game_time_s, our_team: "ORDER" }) as never;

beforeEach(async () => {
  vi.resetModules();
  call.mockReset();
  store = await import("./review.svelte");
});

afterEach(() => {
  vi.useRealTimers();
});

describe("opening a recording", () => {
  it("sets the row before the timeline lands", async () => {
    // The player starts loading the file while the two queries are in flight.
    call.mockResolvedValue([]);
    const open = store.openRecording(row(1));
    expect(store.review.recording?.id).toBe(1);
    await open;
  });

  it("loads markers and samples", async () => {
    call.mockResolvedValueOnce([{ id: 1 }]).mockResolvedValueOnce([sample(10, 0)]);
    await store.openRecording(row(1));
    expect(store.review.markers).toHaveLength(1);
    expect(store.review.samples).toHaveLength(1);
  });

  it("keeps the recording when its timeline fails to load", async () => {
    // A failure costs the timeline, not the video: the recording is the thing
    // the user asked for.
    call.mockRejectedValue(new Error("no daemon"));
    await store.openRecording(row(1));
    expect(store.review.recording?.id).toBe(1);
    expect(store.review.markers).toEqual([]);
  });

  it("discards a slow answer for a recording the user has left", async () => {
    // Two opened in quick succession would otherwise race, and the slower
    // query would win.
    let resolveFirst: (v: unknown) => void = () => {};
    call.mockImplementationOnce(() => new Promise((r) => (resolveFirst = r)));
    call.mockImplementationOnce(() => Promise.resolve([]));
    const first = store.openRecording(row(1));

    call.mockResolvedValue([]);
    await store.openRecording(row(2));

    resolveFirst([{ id: 99 }]);
    await first;

    expect(store.review.recording?.id).toBe(2);
    expect(store.review.markers).toEqual([]);
  });

  it("clears the previous recording's timeline immediately", async () => {
    call.mockResolvedValueOnce([{ id: 1 }]).mockResolvedValueOnce([sample(10, 0)]);
    await store.openRecording(row(1));

    call.mockResolvedValue([]);
    const second = store.openRecording(row(2));
    // Before the queries land: the old markers must not be on screen against
    // the new file.
    expect(store.review.markers).toEqual([]);
    await second;
  });
});

describe("the window", () => {
  it("is empty until the duration is known", async () => {
    call.mockResolvedValue([]);
    await store.openRecording(row(1));
    expect(store.review.window.span).toBe(0);
  });

  it("is measured from the samples once it is", async () => {
    call.mockResolvedValueOnce([]).mockResolvedValueOnce([sample(12, 0), sample(600, 588)]);
    await store.openRecording(row(1));
    store.setDuration(660);

    // Capture ran 12s before the clock started, so the game starts there and
    // the window keeps one second of lead-in.
    expect(store.review.window.start).toBe(11);
    expect(store.review.window.end).toBe(600);
  });

  it("refuses a duration that is not a number", async () => {
    // `video.duration` is NaN until metadata loads; passing it through would
    // make every derived number NaN silently.
    store.setDuration(Number.NaN);
    expect(store.review.duration).toBe(0);
  });
});

describe("closing", () => {
  it("forgets everything", async () => {
    call.mockResolvedValue([{ id: 1 }]);
    await store.openRecording(row(1));
    store.setDuration(100);

    store.closeRecording();
    expect(store.review.recording).toBeNull();
    expect(store.review.markers).toEqual([]);
    expect(store.review.isOpen).toBe(false);
    expect(store.review.duration).toBe(0);
  });
});
