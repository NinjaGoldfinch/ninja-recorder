/**
 * The game open in the review rail beside the player (WS9 P0, moved by P1).
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
import type {
  GameReview,
  Note,
  NoteKind,
  ObjectiveCategory,
  ReviewInput,
  Takeaway,
} from "../contract/types";
import { EMPTY_REVIEW, prefill, type RecordingFacts } from "../reviewform/autofill";
import { createAutosave, type SaveStatus } from "../reviewform/autosave";
import { toast } from "./toast.svelte";

/** How long the form waits after the last change before saving. */
export const AUTOSAVE_DELAY_MS = 600;

let current = $state<GameReview | null>(null);
let draft = $state<ReviewInput>({ ...EMPTY_REVIEW });
/** The fields the form filled in by itself and the user has not changed. */
let auto = $state<(keyof ReviewInput)[]>([]);
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
  /** Whether `field` holds a pre-filled answer the user has not changed. */
  isAuto(field: keyof ReviewInput): boolean {
    return auto.includes(field);
  },
  get loading() {
    return loading;
  },
};

/**
 * Which load is the latest. Opening one VOD after another in quick succession
 * races two loads, and the slower one must not replace the newer game.
 */
let generation = 0;

/**
 * Load the review for a recording, making its game first if it has none.
 *
 * It switches no view: the review lives in the player's rail, so the player
 * opening a recording is what calls this. `facts` is what the recording
 * knows that the game does not, for pre-filling a review never saved
 * (`reviewform/autofill.ts`).
 */
export async function openReviewForRecording(
  recordingId: number,
  facts: RecordingFacts = { deaths: null },
): Promise<void> {
  const mine = ++generation;
  try {
    // Anything unsaved belongs to the game being left, so it is written
    // before that game is replaced.
    await autosave.flush();
    if (mine !== generation) return;
    current = null;
    loading = true;
    const gameId = await client.open_game_for_recording(recordingId);
    if (mine !== generation) return;
    const loaded = await client.get_game_review(gameId);
    if (mine !== generation) return;
    current = loaded;
    // Pre-filled into the draft only: nothing is saved until a real edit.
    const filled = prefill(loaded, facts);
    draft = filled.draft;
    auto = filled.auto;
    status = "saved";
  } catch (err) {
    if (mine === generation) toast(`Couldn't open the review: ${err}`, "error");
  } finally {
    if (mine === generation) loading = false;
  }
}

/** Change part of the review; the whole of it is saved after a pause. */
export function edit(patch: Partial<ReviewInput>): void {
  if (!current) return;
  draft = { ...draft, ...patch };
  auto = auto.filter((field) => !(field in patch));
  autosave.change({ gameId: current.game.id, review: $state.snapshot(draft) });
}

/** Write anything unsaved now: on leaving the form. */
export function flushReview(): Promise<void> {
  return autosave.flush();
}

/** Write anything unsaved, then let the game go: on closing the player. */
export async function closeReview(): Promise<void> {
  const mine = ++generation;
  await autosave.flush();
  if (mine !== generation) return;
  current = null;
  draft = { ...EMPTY_REVIEW };
  auto = [];
  status = "saved";
  loading = false;
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

/** Rewrite a takeaway's text. False, with a toast, if it did not save. */
export async function updateTakeaway(takeawayId: number, body: string): Promise<boolean> {
  if (!current || body.trim() === "") return false;
  const game = current;
  try {
    const updated = await client.update_takeaway(takeawayId, body);
    const row = game.takeaways.find((t) => t.id === takeawayId);
    if (row) row.body = updated.body;
    return true;
  } catch (err) {
    toast(`Couldn't save the takeaway: ${err}`, "error");
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
// --- timed notes (WS9 P1, #258) ---------------------------------------------

/** Game-time order, as the backend returns them, so a new note lands where a
 *  reload would put it. */
function byTime(a: Note, b: Note): number {
  return a.ts_ms - b.ts_ms || a.created_at - b.created_at || a.id - b.id;
}

/**
 * Adds a note at `tsMs` of game time. Resolves to the note, or null when
 * there is no game open or the save failed (which says so in a toast).
 */
export async function addNote(tsMs: number, kind: NoteKind, body: string): Promise<Note | null> {
  if (!current || body.trim() === "") return null;
  const game = current;
  try {
    const added = await client.add_note(game.game.id, Math.max(0, Math.round(tsMs)), kind, body);
    game.notes = [...game.notes, added].sort(byTime);
    return added;
  } catch (err) {
    toast(`Couldn't save the note: ${err}`, "error");
    return null;
  }
}

export async function updateNote(noteId: number, kind: NoteKind, body: string): Promise<boolean> {
  if (!current || body.trim() === "") return false;
  const game = current;
  try {
    const updated = await client.update_note(noteId, kind, body);
    game.notes = game.notes.map((n) => (n.id === noteId ? updated : n));
    return true;
  } catch (err) {
    toast(`Couldn't save the note: ${err}`, "error");
    return false;
  }
}

export async function deleteNote(noteId: number): Promise<void> {
  if (!current) return;
  const game = current;
  try {
    await client.delete_note(noteId);
    game.notes = game.notes.filter((n) => n.id !== noteId);
  } catch (err) {
    toast(`Couldn't delete the note: ${err}`, "error");
  }
}

export function resetGameReviewForTests(): void {
  autosave.cancel();
  generation = 0;
  current = null;
  draft = { ...EMPTY_REVIEW };
  auto = [];
  status = "saved";
  loading = false;
}
