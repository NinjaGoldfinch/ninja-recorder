/**
 * What the review form fills in by itself, from what the recording already
 * knows.
 *
 * **Only a review that has never been saved is filled**, and only in the
 * draft: nothing is written until the user makes a real edit, and then the
 * pre-filled answers are saved with it. A saved review is the user's, blank
 * fields included, so it is never touched.
 *
 * Every pre-filled field is named in `auto`, which is what the form tags
 * "auto" until the user changes it.
 */

import type { GameReview, ReviewInput } from "../contract/types";

export const EMPTY_REVIEW: ReviewInput = {
  game_rating: null,
  lane_rating: null,
  mental_rating: null,
  first_clear_ms: null,
  smites_at_clear: null,
  deaths: null,
  free_notes: "",
};

/** Facts about the recording that the game review itself does not carry. */
export interface RecordingFacts {
  /** Our own deaths, from the end-of-game stats. */
  deaths: number | null;
}

export interface Prefilled {
  draft: ReviewInput;
  auto: (keyof ReviewInput)[];
}

export function prefill(loaded: GameReview | null, facts: RecordingFacts): Prefilled {
  if (loaded?.review) return { draft: { ...loaded.review }, auto: [] };

  const draft: ReviewInput = { ...EMPTY_REVIEW };
  const auto: (keyof ReviewInput)[] = [];
  if (!loaded) return { draft, auto };

  // The result is a fact, and the rating defaults to it; a player who thinks
  // a win was a loss in all but name can say so.
  if (loaded.game.result !== null) {
    draft.game_rating = loaded.game.result;
    auto.push("game_rating");
  }
  // The end-of-game stats rather than the death markers: those are only as
  // complete as the poll that saw them, and a blank box already falls back
  // to them.
  if (facts.deaths !== null && Number.isInteger(facts.deaths) && facts.deaths >= 0) {
    draft.deaths = facts.deaths;
    auto.push("deaths");
  }
  return { draft, auto };
}
