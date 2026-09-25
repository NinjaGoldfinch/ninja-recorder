/**
 * The three rating controls, and the colour each answer wears.
 *
 * The tones follow the spreadsheet the review form replaces: a good answer is
 * green, a middling one amber and a bad one red, whatever the word is. So
 * "win" and "good" share a tone even though they belong to different
 * questions, and the stylesheet only has to know three tones.
 */

import type { GameResult, LaneRating, MentalRating } from "../contract/types";

export type Tone = "good" | "mid" | "bad";

export interface Choice<T extends string> {
  value: T;
  label: string;
  tone: Tone;
}

export const GAME_CHOICES: Choice<GameResult>[] = [
  { value: "win", label: "Win", tone: "good" },
  { value: "loss", label: "Loss", tone: "bad" },
];

export const LANE_CHOICES: Choice<LaneRating>[] = [
  { value: "win", label: "Win", tone: "good" },
  { value: "neutral", label: "Neutral", tone: "mid" },
  { value: "loss", label: "Loss", tone: "bad" },
];

export const MENTAL_CHOICES: Choice<MentalRating>[] = [
  { value: "good", label: "Good", tone: "good" },
  { value: "neutral", label: "Neutral", tone: "mid" },
  { value: "bad", label: "Bad", tone: "bad" },
];

/**
 * The next value when a choice is clicked: the choice, or nothing if it was
 * already chosen. Clicking the lit button again is how a rating is unset.
 */
export function toggle<T extends string>(current: T | null, clicked: T): T | null {
  return current === clicked ? null : clicked;
}
