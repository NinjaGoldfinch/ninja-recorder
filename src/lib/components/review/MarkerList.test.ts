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

function render(markers: MarkerRow[], onseek = () => {}) {
  host = document.createElement("div");
  document.body.append(host);
  instance = mount(MarkerList, { target: host, props: { markers, onseek } });
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
