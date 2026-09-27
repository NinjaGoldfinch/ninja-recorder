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
/** The review rail's game, answered by a fake daemon. */
const client = vi.hoisted(() => ({
  open_game_for_recording: vi.fn(),
  get_game_review: vi.fn(),
  save_game_review: vi.fn(),
}));
vi.mock("../../../bridge", () => ({
  call,
  client,
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

/** Typed loosely on purpose, but with every field the heading and its facts
 *  read, as a real row always has. */
const row = {
  id: 1,
  path: "C:/vods/1.mp4",
  started_at: 0,
  duration_s: null,
  queue: null,
  game_mode: null,
  champion: "Ahri",
  win: null,
  kda_k: null,
  kda_d: null,
  kda_a: null,
  cs: null,
  tier: null,
  division: null,
  lp_after: null,
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
  for (const fn of Object.values(client)) fn.mockReset();
  client.open_game_for_recording.mockResolvedValue(70);
  client.get_game_review.mockResolvedValue({
    game: {
      id: 70,
      recording_id: 1,
      started_at: 0,
      ended_at: null,
      block_id: null,
      champion: "Ahri",
      matchup: null,
      result: "win",
    },
    review: null,
    death_markers: null,
    objectives: [],
    takeaways: [],
  });
  client.save_game_review.mockResolvedValue(null);

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

  it("lists what is known about the game beside the heading", async () => {
    const el = render();
    call.mockResolvedValueOnce([]).mockResolvedValueOnce([]);
    await store.openRecording({ ...row, queue: 420, kda_k: 9, kda_d: 2, kda_a: 11 } as never);
    await Promise.resolve();
    const facts = el.querySelector(".review-facts")?.textContent ?? "";
    expect(facts).toContain("Ranked Solo");
    expect(facts).toContain("9 / 2 / 11");
    // The deaths reach the review too, for pre-filling it.
    await vi.waitFor(() =>
      expect(el.querySelector<HTMLInputElement>("#review-deaths")?.value).toBe("2"),
    );
  });

  it("says nothing about capture for a clean recording", async () => {
    const el = render();
    await open();
    await Promise.resolve();
    expect(el.querySelector(".review-without")).toBeNull();
  });

  /** What the recording lost (#10), stored with it, in full and as text. */
  it("says what the recording is without, with the reason, as text", async () => {
    const el = render();
    const reason = "<i>refused</i> (0x80070005)";
    call.mockResolvedValueOnce([]).mockResolvedValueOnce([]);
    await store.openRecording({
      ...row,
      diagnostics_json: JSON.stringify({
        capture_problems: [{ kind: "sourceFailed", source: "game", reason }],
      }),
    } as never);
    await Promise.resolve();
    const line = el.querySelector(".review-without");
    expect(line?.textContent).toBe(`Recorded without game audio (${reason}).`);
    expect(line?.querySelector("i")).toBeNull();
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

describe("the review rail", () => {
  it("loads the recording's game review beside the player", async () => {
    const el = render();
    await open();
    await vi.waitFor(() => expect(client.get_game_review).toHaveBeenCalledWith(70));
    expect(client.open_game_for_recording).toHaveBeenCalledWith(1);
    expect(el.querySelector(".review-rail #rail-panel-review")?.hasAttribute("hidden")).toBe(false);
    await vi.waitFor(() => expect(el.querySelector('[aria-label="Notes"]')).not.toBeNull());
  });

  it("lists the events in their own tab, and seeks from them", async () => {
    const el = render();
    await open([marker(60), marker(120)]);
    await Promise.resolve();
    const events = el.querySelector<HTMLButtonElement>("#rail-tab-events");
    expect(events?.textContent).toContain("2");
    events?.click();
    await Promise.resolve();
    expect(el.querySelector("#rail-panel-events")?.hasAttribute("hidden")).toBe(false);
    expect(el.querySelector("#rail-panel-review")?.hasAttribute("hidden")).toBe(true);
    el.querySelectorAll<HTMLElement>(".marker-list li")[1]?.click();
    expect(video(el).currentTime).toBe(120);
  });

  it("writes an unsaved review when the player closes", async () => {
    const el = render();
    await open();
    const notes = await vi.waitFor(() => {
      const found = el.querySelector<HTMLTextAreaElement>('[aria-label="Notes"]');
      if (!found) throw new Error("form not loaded");
      return found;
    });
    notes.value = "ward earlier";
    notes.dispatchEvent(new Event("input", { bubbles: true }));
    el.querySelector<HTMLButtonElement>(".back-btn")?.click();
    await vi.waitFor(() =>
      expect(client.save_game_review).toHaveBeenCalledWith(
        70,
        expect.objectContaining({ free_notes: "ward earlier" }),
      ),
    );
  });
});

describe("theatre mode", () => {
  afterEach(() => localStorage.clear());

  const rail = (el: HTMLElement) => el.querySelector<HTMLElement>(".review-rail");

  it("folds the rail away with the button and brings it back with t", async () => {
    const el = render();
    await open();
    await Promise.resolve();
    expect(rail(el)?.hidden).toBe(false);

    el.querySelector<HTMLButtonElement>(".rail-toggle")?.click();
    await Promise.resolve();
    expect(rail(el)?.hidden).toBe(true);
    expect(el.querySelector(".review-layout")?.classList.contains("rail-closed")).toBe(true);
    expect(el.querySelector(".rail-toggle")?.getAttribute("aria-pressed")).toBe("true");

    key("t");
    await Promise.resolve();
    expect(rail(el)?.hidden).toBe(false);
  });

  it("is remembered for the next VOD", async () => {
    let el = render();
    await open();
    key("t");
    await Promise.resolve();
    await svelte.unmount(instance as Record<string, unknown>, { outro: false });
    instance = null;

    el = render();
    await open();
    await Promise.resolve();
    expect(rail(el)?.hidden).toBe(true);
  });

  it("does not fold the rail while typing in it", async () => {
    const el = render();
    await open();
    const notes = await vi.waitFor(() => {
      const found = el.querySelector<HTMLTextAreaElement>('[aria-label="Notes"]');
      if (!found) throw new Error("form not loaded");
      return found;
    });
    notes.focus();
    key("t");
    await Promise.resolve();
    expect(rail(el)?.hidden).toBe(false);
  });
});

describe("notes while watching", () => {
  afterEach(() => localStorage.clear());

  const notesBox = (el: HTMLElement) =>
    vi.waitFor(() => {
      const found = el.querySelector<HTMLTextAreaElement>('[aria-label="Notes"]');
      if (!found) throw new Error("form not loaded");
      return found;
    });

  it("n pauses and starts a note at the playhead, ready to type", async () => {
    const el = render();
    await open([marker(60)]);
    const notes = await notesBox(el);
    video(el).currentTime = 125;
    await video(el).play();
    key("n");
    await vi.waitFor(() => expect(notes.value).toBe("2:05 "));
    expect(video(el).paused).toBe(true);
    expect(document.activeElement).toBe(notes);
  });

  it("opens the rail from theatre mode to take the note", async () => {
    const el = render();
    await open();
    const notes = await notesBox(el);
    key("t");
    await Promise.resolve();
    expect(el.querySelector<HTMLElement>(".review-rail")?.hidden).toBe(true);
    key("n");
    await vi.waitFor(() => expect(notes.value).not.toBe(""));
    expect(el.querySelector<HTMLElement>(".review-rail")?.hidden).toBe(false);
  });

  it("the stamp button does the same as n", async () => {
    const el = render();
    await open();
    const notes = await notesBox(el);
    el.querySelector<HTMLButtonElement>(".stamp-btn")?.click();
    await vi.waitFor(() => expect(notes.value).toBe("0:00 "));
  });

  it("Escape hands the keys back to the player, and Ctrl+Space plays from a field", async () => {
    const el = render();
    await open();
    const notes = await notesBox(el);
    notes.focus();
    document.dispatchEvent(
      new KeyboardEvent("keydown", { key: " ", ctrlKey: true, bubbles: true }),
    );
    await Promise.resolve();
    expect(video(el).paused).toBe(false);

    key("Escape");
    expect(document.activeElement).not.toBe(notes);
    key(" ");
    await Promise.resolve();
    expect(video(el).paused).toBe(true);
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

/**
 * The element's own `duration`, which is not the store's.
 *
 * `setDuration` tells the store how long the recording is; the guards in the
 * component read `video.duration`, and jsdom's inert element leaves that NaN.
 */
function stampDuration(v: HTMLVideoElement, seconds: number) {
  Object.defineProperty(v, "duration", { value: seconds, configurable: true });
}

describe("scrubbing", () => {
  /** jsdom lays nothing out, so the track has to be told how wide it is. */
  function sized(track: HTMLElement, width = 200, left = 0) {
    track.getBoundingClientRect = () =>
      ({
        left,
        width,
        right: left + width,
        top: 0,
        bottom: 10,
        height: 10,
        x: left,
        y: 0,
      }) as DOMRect;
  }

  /** jsdom has no `PointerEvent`, so a MouseEvent carries the two fields the
   *  handler reads. Dispatching a real event rather than calling the handler
   *  keeps Svelte's delegation in the path being tested. */
  function down(track: HTMLElement, clientX: number) {
    const event = new MouseEvent("pointerdown", { clientX, bubbles: true });
    Object.defineProperty(event, "pointerId", { value: 1 });
    track.dispatchEvent(event);
  }

  it("seeks to the fraction of the window the pointer fell at", async () => {
    const el = render();
    await open([], 600);
    await Promise.resolve();
    const track = el.querySelector<HTMLElement>(".player-scrub");
    if (!track) throw new Error("no progress track");
    sized(track);
    stampDuration(video(el), 600);

    down(track, 100);
    const { start, span } = store.review.window;
    expect(video(el).currentTime).toBeCloseTo(start + span * 0.5, 3);
  });

  it("clamps a pointer that left the track rather than seeking outside it", async () => {
    const el = render();
    await open([], 600);
    await Promise.resolve();
    const track = el.querySelector<HTMLElement>(".player-scrub");
    if (!track) throw new Error("no progress track");
    sized(track);
    stampDuration(video(el), 600);

    down(track, -500);
    expect(video(el).currentTime).toBeCloseTo(store.review.window.start, 3);

    down(track, 5000);
    const { start, span } = store.review.window;
    expect(video(el).currentTime).toBeCloseTo(start + span, 3);
  });

  it("does nothing on a track with no width, rather than dividing by zero", async () => {
    const el = render();
    await open([], 600);
    await Promise.resolve();
    const track = el.querySelector<HTMLElement>(".player-scrub");
    if (!track) throw new Error("no progress track");
    sized(track, 0);
    stampDuration(video(el), 600);
    video(el).currentTime = 42;

    down(track, 10);
    expect(video(el).currentTime).toBe(42);
  });

  it("survives a pointer capture that throws, because the seek matters more", async () => {
    const el = render();
    await open([], 600);
    await Promise.resolve();
    const track = el.querySelector<HTMLElement>(".player-scrub");
    if (!track) throw new Error("no progress track");
    sized(track);
    stampDuration(video(el), 600);
    track.setPointerCapture = () => {
      throw new Error("no capture in jsdom");
    };

    down(track, 100);
    expect(video(el).currentTime).toBeGreaterThan(0);
  });
});

describe("the end of the window", () => {
  /**
   * #119: playback stops at the end of the viewing window rather than running
   * into the black tail. Checked from `timeupdate` as well as the rAF loop,
   * because the loop only runs while frames are produced and `timeupdate`
   * keeps firing at ~4 Hz regardless.
   */
  it("pauses and clamps when playback reaches it", async () => {
    const el = render();
    await open([], 600);
    await Promise.resolve();

    const v = video(el);
    Object.defineProperty(v, "paused", { value: false, configurable: true });
    // Longer than the window, or there is no tail to protect and the guard
    // correctly does nothing.
    stampDuration(v, 1000);
    const end = store.review.window.end;
    v.currentTime = end + 2;
    v.dispatchEvent(new Event("timeupdate"));

    // Clamped rather than left a few milliseconds past: "27:20 / 27:18" is
    // the kind of thing that looks like a bug.
    expect(v.currentTime).toBe(end);
  });

  it("leaves playback alone before it gets there", async () => {
    const el = render();
    await open([], 600);
    await Promise.resolve();

    const v = video(el);
    Object.defineProperty(v, "paused", { value: false, configurable: true });
    stampDuration(v, 1000);
    v.currentTime = 5;
    v.dispatchEvent(new Event("timeupdate"));
    expect(v.currentTime).toBe(5);
  });
});

describe("clicking the video", () => {
  /**
   * Restored after WS4.5 dropped it: `review.ts` bound a click handler to the
   * element and the migration rebuilt the `<video>` without one, so the only
   * way to pause was the button or the spacebar. Found on Windows, because
   * nothing in CI clicks anything.
   */
  it("toggles playback", async () => {
    const el = render();
    await open([], 600);
    await Promise.resolve();

    const v = video(el);
    expect(v.paused).toBe(true);
    v.click();
    await Promise.resolve();
    expect(v.paused).toBe(false);

    v.click();
    await Promise.resolve();
    expect(v.paused).toBe(true);
  });

  it("does not toggle on the click that dismisses the settings menu", async () => {
    // The click that closes the menu lands on a frame the user was not aiming
    // at. `review.ts` guarded this with the same flag.
    const el = render();
    await open([], 600);
    await Promise.resolve();

    const gear = el.querySelector<HTMLButtonElement>(".player-menu > button");
    if (!gear) throw new Error("no settings button");
    gear.click();
    await Promise.resolve();

    // On the element, not on `document`: a real pointerdown always targets an
    // element and bubbles, and the handler reads `closest` off the target.
    const v = video(el);
    v.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true }));
    v.click();
    await Promise.resolve();

    expect(v.paused, "the dismissing click also toggled playback").toBe(true);
  });

  it("still toggles on the next click, once the menu is closed", async () => {
    const el = render();
    await open([], 600);
    await Promise.resolve();

    const v = video(el);
    v.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true }));
    v.click();
    await Promise.resolve();
    expect(v.paused).toBe(false);
  });
});
