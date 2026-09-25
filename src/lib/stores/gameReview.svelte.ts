/**
 * The game open in the review form (WS9 P0).
 *
 * **This store moves on command results, not on events.** The rest of the
 * app's state is fed by the daemon's event stream (see `index.ts`), but no
 * review event exists yet: the only writer of a review is this form, in this
 * window. When a second writer appears (P1's notes, P2's widget promoting a
 * takeaway), the review needs an event and this store should follow it.
 *
 * The review itself is edited as a draft and saved whole by `autosave.ts`.
 * Ticks and takeaways are single actions, saved as they happen.
 */

import { client } from "../../bridge";
import { showView } from "../../router";
import type { GameReview, ObjectiveCategory, ReviewInput, Takeaway } from "../contract/types";
import { createAutosave, type SaveStatus } from "../reviewform/autosave";
import { toast } from "./toast.svelte";

/** How long the form waits after the last change before saving. */
export const AUTOSAVE_DELAY_MS = 600;

const EMPTY: ReviewInput = {
  game_rating: null,
  lane_rating: null,
  mental_rating: null,
  first_clear_ms: null,
  smites_at_clear: null,
  deaths: null,
  free_notes: "",
};

let current = $state<GameReview | null>(null);
let draft = $state<ReviewInput>({ ...EMPTY });
let status = $state<SaveStatus>("saved");
let loading = $state(false);

const autosave = createAutosave<{ gameId: number; review: ReviewInput }>({
  save: ({ gameId, review }) => client.save_game_review(gameId, review).then(() => undefined),
  delayMs: AUTOSAVE_DELAY_MS,
  onStatus: (next, error) => {
    status = next;
    if (next === "error") toast(`Couldn't save the review: ${error}`, "error");
  },
});

export const gameReview = {
  get current() {
    return current;
  },
  get draft() {
    return draft;
  },
  get status() {
    return status;
  },
  get loading() {
    return loading;
  },
};

/** Open the review for a recording, making its game first if it has none. */
export async function openReviewForRecording(recordingId: number): Promise<void> {
  try {
    const gameId = await client.open_game_for_recording(recordingId);
    await loadGame(gameId);
    showView("game");
  } catch (err) {
    toast(`Couldn't open the review: ${err}`, "error");
  }
}

export async function loadGame(gameId: number): Promise<void> {
  // Anything unsaved belongs to the game being left, so it is written first.
  await autosave.flush();
  loading = true;
  try {
    const loaded = await client.get_game_review(gameId);
    current = loaded;
    draft = { ...(loaded?.review ?? EMPTY) };
    status = "saved";
  } finally {
    loading = false;
  }
}

/** Change part of the review; the whole of it is saved after a pause. */
export function edit(patch: Partial<ReviewInput>): void {
  if (!current) return;
  draft = { ...draft, ...patch };
  autosave.change({ gameId: current.game.id, review: $state.snapshot(draft) });
}

/** Write anything unsaved now: on leaving the form. */
export function flushReview(): Promise<void> {
  return autosave.flush();
}

export async function closeReview(): Promise<void> {
  await autosave.flush();
  showView("library");
}

/** Optimistic: the box ticks at once and unticks again if the save fails. */
export async function setTicked(objectiveId: number, ticked: boolean): Promise<void> {
  if (!current) return;
  const game = current;
  const flip = (value: boolean) => {
    const row = game.objectives.find((o) => o.objective_id === objectiveId);
    if (row) row.ticked = value;
  };
  flip(ticked);
  try {
    await client.set_objective_ticked(game.game.id, objectiveId, ticked);
  } catch (err) {
    flip(!ticked);
    toast(`Couldn't save that tick: ${err}`, "error");
  }
}

export async function addTakeaway(body: string): Promise<boolean> {
  if (!current || body.trim() === "") return false;
  const game = current;
  try {
    const added = await client.add_takeaway({ kind: "game", id: game.game.id }, body);
    game.takeaways.push(added);
    return true;
  } catch (err) {
    toast(`Couldn't add the takeaway: ${err}`, "error");
    return false;
  }
}

export async function deleteTakeaway(takeawayId: number): Promise<void> {
  if (!current) return;
  const game = current;
  try {
    await client.delete_takeaway(takeawayId);
    game.takeaways = game.takeaways.filter((t) => t.id !== takeawayId);
  } catch (err) {
    toast(`Couldn't delete the takeaway: ${err}`, "error");
  }
}

/** Promote a takeaway to an active objective, and mark it as promoted. */
export async function promoteTakeaway(
  takeawayId: number,
  category: ObjectiveCategory = "other",
): Promise<void> {
  if (!current) return;
  const game = current;
  try {
    const objective = await client.promote_takeaway(takeawayId, category);
    const row: Takeaway | undefined = game.takeaways.find((t) => t.id === takeawayId);
    if (row) row.promoted_to_id = objective.id;
    toast("Promoted to an active objective");
  } catch (err) {
    toast(`Couldn't promote the takeaway: ${err}`, "error");
  }
}

/** Test seam: back to a freshly loaded module. */
export function resetGameReviewForTests(): void {
  autosave.cancel();
  current = null;
  draft = { ...EMPTY };
  status = "saved";
  loading = false;
}
