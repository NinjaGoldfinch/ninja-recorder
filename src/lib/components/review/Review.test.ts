import type { Component } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

/**
 * The imperative island.
 *
 * A `<video>`'s `currentTime` is not state anything should be diffing, so this
 * component holds a real element and talks to it directly. That makes it the
 * hardest part of the frontend to test and the part where a silent mistake
 * costs the most: the hotkeys are the **only** marker navigation available in
 * fullscreen, because the rich timeline is outside `.player-wrap`.
 *
 * jsdom implements no playback, so `test-setup.ts` makes the element inert.
 * What is tested here is the wiring: that a key reaches the right action, that
 * the guards hold, and that closing tears the session down.
 */

const call = vi.hoisted(() => vi.fn());
vi.mock("../../../bridge", () => ({
  call,
  hasDevCommands: vi.fn().mockResolvedValue(false),
  assetUrl: (p: string) => p,
}));
const showView = vi.hoisted(() => vi.fn());
vi.mock("../../../router", () => ({
  showView,
  currentView: () => "review",
  onViewChange: vi.fn(),
  registerView: vi.fn(),
  initRouting: vi.fn(),
}));

let host: HTMLElement;
let instance: Record<string, unknown> | null = null;
let Review: Component;
let store: typeof import("../../stores/review.svelte");
type Svelte = typeof import("svelte");
let svelte: Svelte;

/** Typed loosely on purpose: only these five fields are read here. */
const row = {
  id: 1,
  path: "C:/vods/1.mp4",
  champion: "Ahri",
  audio_tracks_json: null as string | null,
  scoreboard_json: null as string | null,
};

const marker = (video_time_s: number, kind = "kill") =>
  ({
    id: video_time_s,
    recording_id: 1,
    game_time_s: video_time_s,
    video_time_s,
    kind,
    payload_json: "{}",
  }) as never;

beforeEach(async () => {
  vi.resetModules();
  call.mockReset();
  call.mockResolvedValue([]);
  showView.mockReset();

  svelte = await import("svelte");
  store = await import("../../stores/review.svelte");
  Review = (await import("./Review.svelte")).default;

  host = document.createElement("div");
  document.body.append(host);
});

afterEach(async () => {
  if (instance) await svelte.unmount(instance, { outro: false });
  host.remove();
  instance = null;
});

function render() {
  instance = svelte.mount(Review, { target: host });
  return host;
}

/** Opens a recording with `markers` and a known duration. */
async function open(markers: unknown[] = [], durationS = 600) {
  call.mockResolvedValueOnce(markers).mockResolvedValueOnce([]);
  await store.openRecording(row as never);
  store.setDuration(durationS);
  await Promise.resolve();
}

const video = (el: HTMLElement) => el.querySelector("video") as HTMLVideoElement;
const key = (k: string) =>
  document.dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true }));

describe("loading a recording", () => {
  it("points the element at the file", async () => {
    const el = render();
    await open();
    await Promise.resolve();
    expect(video(el).getAttribute("src")).toContain("1.mp4");
  });

  it("shows the long heading, with the matchup", async () => {
    const el = render();
    await open();
    await Promise.resolve();
    expect(el.querySelector("h2")?.textContent).toContain("Ahri");
  });
});

describe("the hotkeys", () => {
  it("seeks with the arrows", async () => {
    const el = render();
    await open();
    await Promise.resolve();
    video(el).currentTime = 100;

    key("ArrowRight");
    expect(video(el).currentTime).toBe(105);
    key("ArrowLeft");
    expect(video(el).currentTime).toBe(100);
  });

  it("jumps between markers", async () => {
    const el = render();
    await open([marker(60), marker(120), marker(300)]);
    await Promise.resolve();
    video(el).currentTime = 0;

    key("]");
    expect(video(el).currentTime).toBe(60);
    key("]");
    expect(video(el).currentTime).toBe(120);
    key("[");
    expect(video(el).currentTime).toBe(60);
  });

  it("jumps between deaths only", async () => {
    const el = render();
    await open([marker(60, "kill"), marker(120, "death"), marker(300, "kill")]);
    await Promise.resolve();
    video(el).currentTime = 0;

    key("D");
    expect(video(el).currentTime).toBe(120);
  });

  it("clamps a seek to the window rather than landing in the tail", async () => {
    // Every seek goes through the same clamp, so nothing can land in the
    // skipped lead or the black tail past the end of the game.
    const el = render();
    await open([]);
    await Promise.resolve();
    video(el).currentTime = 0;

    for (let i = 0; i < 200; i++) key("ArrowRight");
    expect(video(el).currentTime).toBeLessThanOrEqual(store.review.window.end);
  });

  it("does nothing while typing", async () => {
    const el = render();
    await open();
    await Promise.resolve();
    video(el).currentTime = 100;

    const input = document.createElement("input");
    document.body.append(input);
    input.focus();
    key("ArrowRight");
    expect(video(el).currentTime).toBe(100);
    input.remove();
  });

  it("leaves the arrows to a focused select", async () => {
    // A `<select>` uses them; a `<button>` does not, which is why the guard
    // is on selects alone.
    const el = render();
    await open();
    await Promise.resolve();
    video(el).currentTime = 100;

    const select = document.createElement("select");
    document.body.append(select);
    select.focus();
    key("ArrowRight");
    expect(video(el).currentTime).toBe(100);
    select.remove();
  });

  it("still seeks with a focused button", async () => {
    const el = render();
    await open();
    await Promise.resolve();
    video(el).currentTime = 100;

    // Clicking a timeline glyph leaves a button focused, and that used to
    // kill seeking until you clicked elsewhere.
    const button = el.querySelector("button");
    button?.focus();
    key("ArrowRight");
    expect(video(el).currentTime).toBe(105);
  });
});

describe("the controls", () => {
  it("plays and pauses", async () => {
    const el = render();
    await open();
    await Promise.resolve();

    const play = el.querySelector<HTMLButtonElement>('[title^="Play"]');
    play?.click();
    await Promise.resolve();
    expect(el.querySelector('[title^="Pause"]')).not.toBeNull();
  });

  it("restarts from a player parked at the end of the window", async () => {
    // Otherwise play hits `stopAtWindowEnd` on the next frame and pauses
    // again: a button that visibly does nothing.
    const el = render();
    await open();
    await Promise.resolve();
    video(el).currentTime = store.review.window.end;

    el.querySelector<HTMLButtonElement>('[title^="Play"]')?.click();
    await Promise.resolve();
    expect(video(el).currentTime).toBe(store.review.window.start);
  });

  it("mutes and unmutes with the button and the key", async () => {
    const el = render();
    await open();
    await Promise.resolve();

    el.querySelector<HTMLButtonElement>('[title^="Mute"]')?.click();
    await Promise.resolve();
    expect(video(el).muted).toBe(true);

    key("m");
    await Promise.resolve();
    expect(video(el).muted).toBe(false);
  });

  it("treats dragging the slider to zero as muting", async () => {
    const el = render();
    await open();
    await Promise.resolve();

    const slider = el.querySelector<HTMLInputElement>(".volume-slider");
    if (!slider) throw new Error("no slider");
    slider.value = "0";
    slider.dispatchEvent(new Event("input", { bubbles: true }));
    await Promise.resolve();
    expect(video(el).muted).toBe(true);
  });

  it("changes the playback rate", async () => {
    const el = render();
    await open();
    await Promise.resolve();

    const select = el.querySelector<HTMLSelectElement>(".player-menu-panel select");
    if (!select) throw new Error("no rate select");
    select.value = "2";
    select.dispatchEvent(new Event("change", { bubbles: true }));
    await Promise.resolve();
    expect(video(el).playbackRate).toBe(2);
  });

  it("opens and closes the settings menu, and Escape closes it", async () => {
    const el = render();
    await open();
    await Promise.resolve();

    const panel = () => el.querySelector<HTMLElement>(".player-menu-panel");
    expect(panel()?.hidden).toBe(true);

    el.querySelector<HTMLButtonElement>('[title="Settings"]')?.click();
    await Promise.resolve();
    expect(panel()?.hidden).toBe(false);

    // Guarded on the menu being open, so it never shadows the user agent's
    // own Escape-exits-fullscreen.
    key("Escape");
    await Promise.resolve();
    expect(panel()?.hidden).toBe(true);
  });
});

describe("audio tracks", () => {
  const layout = JSON.stringify({
    tracks: [{ label: "Everything" }, { label: "Game" }, { label: "Mic" }],
  });

  it("offers the picker only for a recording with stems", async () => {
    const el = render();
    await open();
    await Promise.resolve();
    expect(el.querySelector<HTMLElement>("label[hidden]")).not.toBeNull();

    call.mockResolvedValueOnce([]).mockResolvedValueOnce([]);
    await store.openRecording({ ...row, id: 2, audio_tracks_json: layout } as never);
    store.setDuration(600);
    await Promise.resolve();
    expect(el.querySelector<HTMLSelectElement>('[aria-label="Audio track"]')).not.toBeNull();
  });

  it("extracts a stem for any track but the mix, and mutes the video", async () => {
    // Track 0 is the combined mix and plays straight off the element;
    // WebView2 gives no way to select among one `<video>`'s audio tracks.
    const el = render();
    call.mockResolvedValueOnce([]).mockResolvedValueOnce([]);
    await store.openRecording({ ...row, audio_tracks_json: layout } as never);
    store.setDuration(600);
    await Promise.resolve();

    call.mockResolvedValueOnce("C:/vods/1.track2.m4a");
    const select = el.querySelector<HTMLSelectElement>('[aria-label="Audio track"]');
    if (!select) throw new Error("no track picker");
    select.value = "2";
    select.dispatchEvent(new Event("change", { bubbles: true }));
    await Promise.resolve();
    await Promise.resolve();

    expect(call).toHaveBeenCalledWith("extract_audio_track", {
      recordingPath: "C:/vods/1.mp4",
      trackIndex: 2,
    });
    // Or the combined mix and the isolated stem would play on top of each
    // other.
    expect(video(el).muted).toBe(true);
  });
});

describe("closing", () => {
  it("tears the session down and goes back to the library", async () => {
    const el = render();
    await open([marker(60)]);
    await Promise.resolve();

    el.querySelector<HTMLButtonElement>(".back-btn")?.click();
    await Promise.resolve();

    expect(store.review.isOpen).toBe(false);
    expect(showView).toHaveBeenCalledWith("library");
    expect(video(el).getAttribute("src")).toBeNull();
  });
});
