import type { Component } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { GameReview, ReviewInput, Takeaway } from "../../contract/types";

/**
 * The review form against a fake daemon: the typed client is replaced by an
 * in-memory one, so what is asserted is what the form *sends*, which is the
 * contract the daemon's own tests pin from the other side.
 */

const client = vi.hoisted(() => ({
  open_game_for_recording: vi.fn(),
  get_game_review: vi.fn(),
  save_game_review: vi.fn(),
  set_objective_ticked: vi.fn(),
  add_takeaway: vi.fn(),
  delete_takeaway: vi.fn(),
  promote_takeaway: vi.fn(),
}));
vi.mock("../../../bridge", () => ({ client, call: vi.fn(), assetUrl: (p: string) => p }));
const showView = vi.hoisted(() => vi.fn());
vi.mock("../../../router", () => ({
  showView,
  currentView: () => "game",
  onViewChange: vi.fn(),
  registerView: vi.fn(),
}));

function fixture(over: Partial<GameReview> = {}): GameReview {
  return {
    game: {
      id: 7,
      recording_id: 3,
      started_at: Date.UTC(2026, 8, 16, 5, 23),
      ended_at: null,
      block_id: 1,
      champion: "Lee Sin",
      matchup: "Vi",
      result: "loss",
    },
    review: null,
    death_markers: 7,
    objectives: [
      {
        objective_id: 1,
        body: "Ward river at 2:45",
        category: "macro",
        status: "active",
        ticked: false,
      },
      {
        objective_id: 2,
        body: "Track the enemy jungler",
        category: "lane",
        status: "retired",
        ticked: true,
      },
    ],
    takeaways: [],
    ...over,
  };
}

let host: HTMLElement;
let instance: Record<string, unknown> | null = null;
type Svelte = typeof import("svelte");
let svelte: Svelte;
let store: typeof import("../../stores/gameReview.svelte");
let ReviewForm: Component;

const settle = async () => {
  for (let i = 0; i < 5; i++) await new Promise((r) => setTimeout(r, 0));
};

beforeEach(async () => {
  vi.resetModules();
  for (const fn of Object.values(client)) fn.mockReset();
  client.get_game_review.mockResolvedValue(fixture());
  client.open_game_for_recording.mockResolvedValue(7);
  client.save_game_review.mockResolvedValue(null);
  client.set_objective_ticked.mockResolvedValue(null);
  client.delete_takeaway.mockResolvedValue(null);
  showView.mockReset();

  svelte = await import("svelte");
  store = await import("../../stores/gameReview.svelte");
  ReviewForm = (await import("./ReviewForm.svelte")).default;
  host = document.createElement("div");
  document.body.append(host);
});

afterEach(async () => {
  if (instance) await svelte.unmount(instance, { outro: false });
  host.remove();
  instance = null;
});

async function open(): Promise<HTMLElement> {
  await store.openReviewForRecording(3);
  instance = svelte.mount(ReviewForm, { target: host });
  await settle();
  return host;
}

function lastSaved(): ReviewInput {
  const calls = client.save_game_review.mock.calls;
  return calls[calls.length - 1][1] as ReviewInput;
}

function radio(el: HTMLElement, group: string, label: string): HTMLButtonElement {
  const button = [...el.querySelectorAll<HTMLButtonElement>(`[aria-label="${group}"] button`)].find(
    (b) => b.textContent === label,
  );
  if (!button) throw new Error(`no ${label} in ${group}`);
  return button;
}

function type(input: HTMLInputElement | HTMLTextAreaElement, value: string) {
  input.value = value;
  input.dispatchEvent(new Event("input", { bubbles: true }));
}

describe("opening a review", () => {
  it("makes the recording's game, loads it and shows the form", async () => {
    const el = await open();
    expect(client.open_game_for_recording).toHaveBeenCalledWith(3);
    expect(client.get_game_review).toHaveBeenCalledWith(7);
    expect(showView).toHaveBeenCalledWith("game");
    expect(el.textContent).toContain("Lee Sin");
    expect(el.textContent).toContain("vs Vi");
    expect(el.querySelector(".result-pill")?.getAttribute("data-outcome")).toBe("loss");
  });

  it("renders a user's text as text, never as markup", async () => {
    client.get_game_review.mockResolvedValue(
      fixture({
        takeaways: [
          {
            id: 1,
            game_id: 7,
            block_id: null,
            body: "<img src=x onerror=alert(1)>",
            objective_id: null,
            promoted_to_id: null,
            created_at: 0,
          },
        ],
      }),
    );
    const el = await open();
    expect(el.querySelector(".review-takeaways img")).toBeNull();
    expect(el.querySelector(".takeaway-body")?.textContent).toBe("<img src=x onerror=alert(1)>");
  });
});

describe("ratings", () => {
  it("lights the chosen answer in its tone and saves the whole review", async () => {
    const el = await open();
    radio(el, "Lane", "Neutral").click();
    await settle();
    expect(radio(el, "Lane", "Neutral").getAttribute("aria-checked")).toBe("true");
    expect(radio(el, "Lane", "Neutral").dataset.tone).toBe("mid");
    expect(el.querySelector(".save-status")?.textContent).toBe("Unsaved changes");

    await store.flushReview();
    expect(client.save_game_review).toHaveBeenCalledWith(
      7,
      expect.objectContaining({ lane_rating: "neutral" }),
    );
    await settle();
    expect(el.querySelector(".save-status")?.textContent).toBe("Saved");
  });

  it("unsets a rating when the lit answer is clicked again", async () => {
    client.get_game_review.mockResolvedValue(
      fixture({
        review: {
          game_rating: "loss",
          lane_rating: null,
          mental_rating: "bad",
          first_clear_ms: null,
          smites_at_clear: null,
          deaths: null,
          free_notes: "",
        },
      }),
    );
    const el = await open();
    expect(radio(el, "Mental", "Bad").getAttribute("aria-checked")).toBe("true");
    radio(el, "Mental", "Bad").click();
    await store.flushReview();
    expect(lastSaved().mental_rating).toBeNull();
    expect(lastSaved().game_rating, "the rest of the review is sent unchanged").toBe("loss");
  });
});

describe("the number fields", () => {
  it("saves a clear time typed as m:ss in milliseconds", async () => {
    const el = await open();
    const [clear] = el.querySelectorAll<HTMLInputElement>(".review-numbers input");
    type(clear, "2:58");
    await store.flushReview();
    expect(lastSaved().first_clear_ms).toBe(178_000);
  });

  it("marks a clear time it cannot read and does not save it", async () => {
    const el = await open();
    const [clear] = el.querySelectorAll<HTMLInputElement>(".review-numbers input");
    type(clear, "2:5x");
    await settle();
    expect(clear.getAttribute("aria-invalid")).toBe("true");
    await store.flushReview();
    expect(client.save_game_review).not.toHaveBeenCalled();
  });

  it("offers the death markers as the default, and a blank box goes back to them", async () => {
    const el = await open();
    const deaths = el.querySelectorAll<HTMLInputElement>(".review-numbers input")[2];
    expect(deaths.placeholder).toBe("7 (auto)");

    type(deaths, "5");
    await store.flushReview();
    expect(lastSaved().deaths).toBe(5);
    type(deaths, "");
    await store.flushReview();
    expect(lastSaved().deaths, "null means use the markers, not zero").toBeNull();
  });
});

describe("reviewing against objectives", () => {
  it("lists what the game was played against, retired ones included, and saves a tick", async () => {
    const el = await open();
    const boxes = el.querySelectorAll<HTMLInputElement>(".review-checklist input");
    expect(boxes).toHaveLength(2);
    expect(el.querySelector(".review-checklist")?.textContent).toContain("(retired)");

    boxes[0].click();
    await settle();
    expect(client.set_objective_ticked).toHaveBeenCalledWith(7, 1, true);
    expect(boxes[0].checked).toBe(true);
  });

  it("unticks again if the tick could not be saved", async () => {
    client.set_objective_ticked.mockRejectedValue("not connected");
    const el = await open();
    const box = el.querySelector<HTMLInputElement>(".review-checklist input");
    box?.click();
    await settle();
    expect(box?.checked).toBe(false);
  });
});

describe("takeaways", () => {
  const takeaway = (id: number, over: Partial<Takeaway> = {}): Takeaway => ({
    id,
    game_id: 7,
    block_id: null,
    body: `takeaway ${id}`,
    objective_id: null,
    promoted_to_id: null,
    created_at: id,
    ...over,
  });

  it("adds one to this game and clears the box", async () => {
    client.add_takeaway.mockResolvedValue(takeaway(9, { body: "Contest grubs with prio" }));
    const el = await open();
    const box = el.querySelector<HTMLTextAreaElement>('[aria-label="New takeaway"]');
    if (!box) throw new Error("no takeaway box");
    type(box, "Contest grubs with prio");
    await settle();
    el.querySelector<HTMLFormElement>(".takeaway-add")?.requestSubmit();
    await settle();

    expect(client.add_takeaway).toHaveBeenCalledWith(
      { kind: "game", id: 7 },
      "Contest grubs with prio",
    );
    expect(el.querySelector(".takeaway-body")?.textContent).toBe("Contest grubs with prio");
    expect(box.value).toBe("");
  });

  it("promotes one and then shows it as promoted", async () => {
    client.get_game_review.mockResolvedValue(fixture({ takeaways: [takeaway(1)] }));
    client.promote_takeaway.mockResolvedValue({ id: 40 });
    const el = await open();
    const promote = [...el.querySelectorAll("button")].find(
      (b) => b.textContent === "Promote to objective",
    );
    promote?.click();
    await settle();
    expect(client.promote_takeaway).toHaveBeenCalledWith(1, "other");
    expect(el.querySelector(".review-takeaways")?.textContent).toContain("Promoted");
  });

  it("deletes one", async () => {
    client.get_game_review.mockResolvedValue(fixture({ takeaways: [takeaway(1), takeaway(2)] }));
    const el = await open();
    el.querySelector<HTMLButtonElement>('[aria-label="Delete takeaway"]')?.click();
    await settle();
    expect(client.delete_takeaway).toHaveBeenCalledWith(1);
    expect(el.querySelectorAll(".takeaway-body")).toHaveLength(1);
  });
});

describe("leaving the form", () => {
  it("writes anything unsaved before going back", async () => {
    const el = await open();
    const notes = el.querySelector<HTMLTextAreaElement>('[aria-label="Notes"]');
    if (!notes) throw new Error("no notes");
    type(notes, "tilted after first death");
    [...el.querySelectorAll("button")].find((b) => b.textContent?.includes("Back"))?.click();
    await settle();
    expect(lastSaved().free_notes).toBe("tilted after first death");
    expect(showView).toHaveBeenLastCalledWith("library");
  });
});
