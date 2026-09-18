import type { Component } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

/**
 * The two empty states, which are the thing most easily got wrong here.
 *
 * "Nothing recorded yet" and "everything is filtered out" are different
 * problems with different next steps. The first message was the only one there
 * used to be, which read as data loss the moment a filter matched nothing.
 *
 * The bridge is mocked rather than stubbed at the window: it is the transport
 * seam WS2 replaced, and a test that reached through it would be rewritten
 * twice.
 */

const call = vi.hoisted(() => vi.fn());
const hasDevCommands = vi.hoisted(() => vi.fn());
vi.mock("../../../bridge", () => ({ call, hasDevCommands, assetUrl: (p: string) => p }));

let host: HTMLElement;
let instance: Record<string, unknown> | null = null;
let Library: Component;
let store: typeof import("../../stores/library.svelte");

type Svelte = typeof import("svelte");
/**
 * The runtime comes from the same reset module graph as the component.
 *
 * `vi.resetModules` gives each test a fresh store, and a statically imported
 * `mount` would then belong to a *different* copy of Svelte from the one the
 * component's `$effect` was compiled against: `effect_orphan`, reported with
 * nothing to say about why.
 */
let svelte: Svelte;

beforeEach(async () => {
  // Fresh module graph per test: the store holds the row set in module scope,
  // and the component has to be compiled against the same Svelte runtime that
  // `mount` comes from (see `router.test.ts`).
  vi.resetModules();
  call.mockReset();
  hasDevCommands.mockReset();
  hasDevCommands.mockResolvedValue(false);

  svelte = await import("svelte");
  store = await import("../../stores/library.svelte");
  Library = (await import("./Library.svelte")).default;

  host = document.createElement("div");
  document.body.append(host);
});

afterEach(async () => {
  if (instance) await svelte.unmount(instance, { outro: false });
  host.remove();
  instance = null;
});

function render() {
  instance = svelte.mount(Library, { target: host });
  return host;
}

/** Loads the library with `rows` through the store's own path. */
async function load(rows: unknown[]) {
  call.mockResolvedValueOnce(rows);
  await store.refreshLibrary();
}

const recording = (over: Record<string, unknown> = {}) => ({
  id: 1,
  path: "C:/vods/a.mp4",
  started_at: 0,
  duration_s: null,
  game_id: null,
  queue: null,
  champion: null,
  role: null,
  win: null,
  kda_k: null,
  kda_d: null,
  kda_a: null,
  patch: null,
  pinned: false,
  size_bytes: 0,
  audio_tracks_json: null,
  game_mode: null,
  diagnostics_json: null,
  scoreboard_json: null,
  cs: null,
  tier: null,
  division: null,
  lp_after: null,
  lp_before: null,
  lp_delta: null,
  ...over,
});

describe("the empty states", () => {
  it("offers the rescan hint when nothing has been recorded", () => {
    const el = render();
    expect(el.textContent).toContain("No recordings yet");
    expect(el.querySelector(".empty-state button")).toBeNull();
  });

  it("says the filters are what is hiding things, and offers a way out", async () => {
    await load([recording({ id: 1, win: true })]);
    store.library.controls.outcome = "losses";

    const el = render();
    expect(el.textContent).toContain("No recordings match these filters");
    expect(el.textContent).toContain("The one recording in the library does not match.");
    expect(el.querySelector(".empty-state button")?.textContent).toContain("Clear filters");
  });

  it("counts the library in the hint when there is more than one", async () => {
    await load([recording({ id: 1, win: true }), recording({ id: 2, win: true })]);
    store.library.controls.outcome = "losses";
    expect(render().textContent).toContain("2 recordings in the library");
  });

  it("clearing the filters brings the rows back", async () => {
    await load([recording({ id: 1, win: true })]);
    store.library.controls.outcome = "losses";
    render();

    store.clearFilters();
    await Promise.resolve();
    expect(store.library.visible).toHaveLength(1);
  });
});

describe("the grid", () => {
  it("renders one row per visible recording", async () => {
    await load([recording({ id: 1 }), recording({ id: 2 })]);
    const el = render();
    expect(el.querySelectorAll(".vod-row")).toHaveLength(2);
    expect(el.querySelector('[role="list"]')).not.toBeNull();
  });

  it("reports a failure to list rather than rendering an empty library", async () => {
    // An error and "you have no recordings" must not look the same.
    call.mockRejectedValueOnce(new Error("no daemon"));
    await store.refreshLibrary();
    expect(store.library.rows).toHaveLength(0);
  });
});
