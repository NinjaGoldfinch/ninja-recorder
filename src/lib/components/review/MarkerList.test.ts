import { mount, unmount } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { MarkerRow } from "../../../types";
import MarkerList from "./MarkerList.svelte";

/**
 * **Payload strings carry other players' names.** `review.ts` built these rows
 * by concatenating HTML and escaping by hand; a champion name is not
 * attacker-controlled, but the escaping was there because the payload is not
 * this app's text. The last test is what keeps that true now it is a template.
 */

const marker = (over: Partial<MarkerRow> = {}): MarkerRow =>
  ({
    id: 1,
    recording_id: 1,
    game_time_s: 90,
    video_time_s: 100,
    kind: "kill",
    payload_json: JSON.stringify({ victim: "Zed" }),
    ...over,
  }) as MarkerRow;

let host: HTMLElement | null = null;
let instance: Record<string, unknown> | null = null;

function render(
  markers: MarkerRow[],
  onseek = (_t: number) => {},
  beyond: MarkerRow[] = [],
  currentTimeS = -1,
) {
  host = document.createElement("div");
  document.body.append(host);
  instance = mount(MarkerList, { target: host, props: { markers, beyond, onseek, currentTimeS } });
  return host;
}

afterEach(async () => {
  if (instance) await unmount(instance, { outro: false });
  host?.remove();
  instance = null;
  host = null;
});

describe("MarkerList", () => {
  it("says so when a game has no markers", () => {
    const el = render([]);
    expect(el.textContent).toContain("No markers recorded for this game.");
    expect(el.querySelectorAll("li")).toHaveLength(1);
  });

  it("renders one row per marker, with its label", () => {
    const el = render([
      marker({ id: 1, payload_json: JSON.stringify({ victim: "Zed" }) }),
      marker({ id: 2, kind: "baron", payload_json: JSON.stringify({ stolen: true }) }),
    ]);
    expect(el.querySelectorAll("li")).toHaveLength(2);
    expect(el.textContent).toContain("Killed Zed");
    expect(el.textContent).toContain("Baron (stolen)");
  });

  it("shows both clocks", () => {
    // The pair is the only place a bad alignment is visible: a kill the list
    // calls 1:30 that seeks to black is a story the two numbers tell together.
    const el = render([marker({ game_time_s: 90, video_time_s: 100 })]);
    expect(el.querySelector(".marker-game-time")?.textContent).toBe("1:30");
    expect(el.textContent).toContain("1:40");
  });

  it("seeks to the marker's video time, not its game time", () => {
    const onseek = vi.fn();
    const el = render([marker({ game_time_s: 90, video_time_s: 100 })], onseek);
    el.querySelector("li")?.click();
    expect(onseek).toHaveBeenCalledWith(100);
  });

  it("renders a payload name as text", () => {
    const el = render([
      marker({ payload_json: JSON.stringify({ victim: "<img src=x onerror=alert(1)>" }) }),
    ]);
    expect(el.querySelectorAll("img")).toHaveLength(0);
    expect(el.querySelector(".marker-label")?.textContent).toBe(
      "Killed <img src=x onerror=alert(1)>",
    );
  });
});

describe("markers the recording does not reach", () => {
  /**
   * A crashed recording ends before the game does, and its markers do not know
   * that: `video_time_s` is provisional until a finalize re-resolves it, and a
   * killed daemon never reaches one. The events are real, so they are listed;
   * the file has no frame for them, so they are not clickable.
   */
  const inside = marker({ id: 1, video_time_s: 10 });
  const outside = marker({ id: 2, video_time_s: 900 });

  it("lists them, marked, alongside the rest", () => {
    const el = render([inside, outside], () => {}, [outside]);
    const rows = el.querySelectorAll("li");
    expect(rows).toHaveLength(2);
    expect(rows[0].classList.contains("beyond-footage")).toBe(false);
    expect(rows[1].classList.contains("beyond-footage")).toBe(true);
  });

  it("does not seek to one, because there is nothing there", () => {
    const seeks: number[] = [];
    const el = render([outside], (t) => seeks.push(t), [outside]);
    el.querySelector("li")?.click();
    expect(seeks).toEqual([]);
  });

  it("still seeks to the ones it does reach", () => {
    const seeks: number[] = [];
    const el = render([inside, outside], (t) => seeks.push(t), [outside]);
    el.querySelectorAll("li")[0].click();
    expect(seeks).toEqual([10]);
  });

  it("says why, and says the times are approximate", () => {
    const el = render([outside], () => {}, [outside]);
    // Whitespace-normalised: the template wraps, so asserting on the raw
    // textContent would be testing where the line breaks fall.
    const note = (el.querySelector(".marker-list-note")?.textContent ?? "").replace(/\s+/g, " ");
    expect(note).toContain("after this recording ends");
    expect(note).toContain("approximate");
  });

  it("says nothing at all when the file reaches everything", () => {
    const el = render([inside]);
    expect(el.querySelector(".marker-list-note")).toBeNull();
    expect(el.querySelector(".beyond-footage")).toBeNull();
  });
});

describe("the Events tab", () => {
  const grubs = (id: number, t: number) =>
    marker({ id, kind: "voidgrubs", game_time_s: t, video_time_s: t + 20, payload_json: "{}" });
  const death = (id: number, t: number) =>
    marker({
      id,
      kind: "death",
      game_time_s: t,
      video_time_s: t + 20,
      payload_json: JSON.stringify({ killer: "Zed" }),
    });

  it("collapses a burst of the same objective into one row", () => {
    const el = render([grubs(1, 649), grubs(2, 656), grubs(3, 660)]);
    const rows = el.querySelectorAll(".marker-list li");
    expect(rows).toHaveLength(1);
    expect(rows[0].textContent).toContain("Voidgrubs ×3");
  });

  it("filters to one kind of event, and counts each", async () => {
    const el = render([grubs(1, 300), death(2, 400), death(3, 700)]);
    const deaths = [...el.querySelectorAll<HTMLButtonElement>(".event-filter")].find((b) =>
      b.textContent?.startsWith("Deaths"),
    );
    expect(deaths?.textContent).toContain("2");
    deaths?.click();
    await Promise.resolve();
    expect(el.querySelectorAll(".marker-list li")).toHaveLength(2);
    expect(el.textContent).not.toContain("Voidgrubs");
  });

  it("lights the row the playhead most recently passed", () => {
    const el = render([death(1, 100), death(2, 400)], () => {}, [], 300);
    const rows = el.querySelectorAll(".marker-list li");
    expect(rows[0].classList.contains("current")).toBe(true);
    expect(rows[0].getAttribute("aria-current")).toBe("true");
    expect(rows[1].classList.contains("current")).toBe(false);
  });

  it("offers no filters for a game with no markers", () => {
    const el = render([]);
    expect(el.querySelector(".event-filters")).toBeNull();
  });
});

describe("a note row", () => {
  const placed = (id: number, videoTimeS: number) => ({
    note: {
      id,
      game_id: 1,
      ts_ms: videoTimeS * 1000,
      kind: "note" as const,
      body: `note ${id}`,
      created_at: 0,
    },
    videoTimeS,
  });

  function renderNotes(onseek = (_t: number) => {}) {
    host = document.createElement("div");
    document.body.append(host);
    instance = mount(MarkerList, {
      target: host,
      props: { markers: [], onseek, notes: [placed(1, 10), placed(2, 20)] },
    });
    return host;
  }

  const rows = (el: HTMLElement) => [...el.querySelectorAll<HTMLElement>(".note-row")];

  it("opens in full on the click that seeks to it, one at a time", async () => {
    const seek = vi.fn();
    const el = renderNotes(seek);
    expect(rows(el).some((r) => r.classList.contains("expanded"))).toBe(false);

    rows(el)[0]?.click();
    await Promise.resolve();
    expect(seek).toHaveBeenCalledWith(10);
    expect(rows(el).map((r) => r.classList.contains("expanded"))).toEqual([true, false]);

    rows(el)[1]?.click();
    await Promise.resolve();
    expect(rows(el).map((r) => r.classList.contains("expanded"))).toEqual([false, true]);

    // A second click on the open one closes it again.
    rows(el)[1]?.click();
    await Promise.resolve();
    expect(rows(el).some((r) => r.classList.contains("expanded"))).toBe(false);
  });
});
