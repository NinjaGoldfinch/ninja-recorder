/**
 * The viewing window: which slice of the file the player treats as the game.
 *
 * Extracted from `review.ts` by WS4.2, unchanged in behaviour. Everything
 * here was previously a module-level function reading module-level mutable
 * state (`gameStartsAt`, `gameEndsAt`, the `<video>` element), which is why
 * none of it had a test: exercising `windowEnd` meant constructing a review
 * view. Taking the state as arguments is the whole of the change.
 *
 * A recording is not the same thing as a game. Capture starts on the loading
 * screen and stops after the end-of-game screen, so the file has a lead-in
 * nobody wants to watch and a tail of post-game UI. The window is what the
 * timeline measures against, what the ruler reads 0:00 at, and what every
 * seek is clamped to.
 */

import type { SampleRow } from "../../types";

/**
 * How much of the loading screen to keep in front of the game.
 *
 * Not zero: cutting to the exact frame the clock starts on opens a VOD
 * mid-fade with no sense of where it began. One second, matching
 * `trim::LEAD_IN_S`, so a trimmed recording and an untrimmed one open at
 * the same place — the alignment is measured from a 1 Hz poll and is only
 * accurate to about that anyway.
 */
export const LEAD_IN_S = 1;

/** Below this there is no loading screen worth skipping. */
export const MIN_SKIP_S = 3;

/**
 * How much to keep after the last thing the game reported: nothing.
 *
 * Capture outlives the game window — nothing stops it at the instant the
 * game ends, because neither signal that ends a recording knows at that
 * instant (#119) — and a window that no longer exists captures as *black*
 * under WGC, not as a frozen last frame.
 *
 * **Deliberately not the mirror of `LEAD_IN_S`.** A margin was kept here so
 * the final moment could not be clipped by the 1 Hz sample cadence, and two
 * seconds was not enough to stop the VOD ending on black anyway. The two ends
 * are not worth the same: the head margin buys the opening of a game, while
 * everything after the last report is the post-game end screen. Losing up to
 * a second of that costs nothing a person would go back for, and ending on
 * black is a defect people actually notice.
 *
 * `viewingWindow` is untouched by this — a gap wider than
 * `MAX_TAIL_CLIP_S` is still refused, so a stretch with no samples cannot
 * cut real gameplay.
 */
export const TAIL_OUT_S = 0;

/**
 * Past this, the gap is not a post-game tail and clipping it would be a
 * guess.
 *
 * The tail is normally 5-15 s: five failed polls at 1 Hz, or up to three
 * times that if the dying game process makes them time out rather than
 * refuse. A much larger gap means something else — most likely a stretch
 * where Live Client Data answered with something the parser could not read,
 * which keeps recording and produces *no samples*, so real gameplay sits
 * after the last one. Cutting there would hide the game.
 *
 * The same rule `trim.rs` applies at the other end: act on a measured
 * answer, never on a guessed one.
 */
export const MAX_TAIL_CLIP_S = 60;

export function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

/**
 * Where the game starts in the file, from the sample with the lowest game
 * time.
 *
 * Returns 0 rather than a negative offset when capture started *after* the
 * game did, which is a reconnect: there is no loading screen in front of it
 * to skip. Also 0 when there is nothing to measure from.
 */
export function measureGameStart(samples: readonly SampleRow[]): number {
  const earliest = samples.reduce<SampleRow | null>(
    (best, s) => (best === null || s.game_time_s < best.game_time_s ? s : best),
    null,
  );
  if (!earliest) return 0;
  const offset = earliest.video_time_s - earliest.game_time_s;
  return offset >= MIN_SKIP_S ? offset : 0;
}

/**
 * Where the game ends in the file: the last sample's video position, or
 * `null` when there are no samples.
 *
 * `null` is not zero and the difference matters. It means "not measured",
 * and `viewingWindow` falls back to the end of the file rather than clipping
 * to a guess.
 */
export function measureGameEnd(samples: readonly SampleRow[]): number | null {
  let latest: number | null = null;
  for (const s of samples) {
    if (latest === null || s.video_time_s > latest) latest = s.video_time_s;
  }
  return latest;
}

/**
 * Whether the recording's stored diagnostics say the Live Client poll was
 * **unreadable after the last sample** (#305).
 *
 * When it was, the stretch between the last sample and the end of the file
 * is game the parser could not read, not the post-game screen, and the
 * player must not clip it: `trim.rs` keeps it in the file for the same
 * reason. The caller passes `null` for `gameEndsAt` instead.
 *
 * `false` for anything that does not say `true` - a row from before this was
 * stored, one a rescan imported, or a blob that does not parse - so those
 * keep the window they always had.
 */
export function unreadableAtEnd(diagnosticsJson: string | null | undefined): boolean {
  if (!diagnosticsJson) return false;
  try {
    const parsed: unknown = JSON.parse(diagnosticsJson);
    return (
      typeof parsed === "object" &&
      parsed !== null &&
      (parsed as { unreadable_at_end?: unknown }).unreadable_at_end === true
    );
  } catch {
    return false;
  }
}

export interface ViewingWindow {
  start: number;
  end: number;
  span: number;
}

/**
 * The slice of `fileEnd` seconds the player shows.
 *
 * **Three ways this declines to clip the tail, and all of them fall back to
 * the end of the file rather than to a guess**: no samples to measure from,
 * a tail already shorter than the margin, or a gap too large to be a
 * post-game tail at all.
 *
 * `fileEnd` must be a finite duration. Callers hold a `<video>` whose
 * `duration` is `NaN` until metadata loads, and passing that through would
 * make every derived number `NaN` silently, so it is refused here with an
 * empty window instead.
 */
export function viewingWindow(
  gameStartsAt: number,
  gameEndsAt: number | null,
  fileEnd: number,
): ViewingWindow {
  if (!Number.isFinite(fileEnd) || fileEnd <= 0) return { start: 0, end: 0, span: 0 };

  const start = Math.max(0, gameStartsAt - LEAD_IN_S);

  let end = fileEnd;
  if (gameEndsAt !== null) {
    const clipped = gameEndsAt + TAIL_OUT_S;
    if (clipped < fileEnd && fileEnd - clipped <= MAX_TAIL_CLIP_S) {
      end = Math.max(start, clipped);
    }
  }

  return { start, end, span: Math.max(0, end - start) };
}

/** Video time to the position shown to the user, where 0 is the window's start. */
export function displayTime(videoTime: number, window: ViewingWindow): number {
  return Math.max(0, videoTime - window.start);
}

/**
 * Fraction across the window, for anything drawn along the timeline.
 *
 * Clamped, so a marker at a position the window excludes is drawn at the edge
 * rather than outside the track.
 */
export function windowFraction(videoTime: number, window: ViewingWindow): number {
  if (window.span <= 0) return 0;
  return clamp((videoTime - window.start) / window.span, 0, 1);
}
