/**
 * The game clock at a position in the recording.
 *
 * The player knows where it is in the *file*; a review asks for game time
 * ("cleared at 3:12"). Every sample and every marker carries both clocks, so
 * the nearest one gives the offset between them. The nearest rather than
 * any one: the offset can drift across a game, and a pause moves it outright.
 */

interface Clocked {
  game_time_s: number;
  video_time_s: number;
}

/** Null when nothing carries both clocks: there is no offset to apply. */
export function gameClockAt(videoTimeS: number, ...sources: readonly Clocked[][]): number | null {
  let nearest: Clocked | null = null;
  for (const source of sources) {
    for (const point of source) {
      if (
        nearest === null ||
        Math.abs(point.video_time_s - videoTimeS) < Math.abs(nearest.video_time_s - videoTimeS)
      ) {
        nearest = point;
      }
    }
  }
  if (nearest === null) return null;
  return Math.max(0, videoTimeS - (nearest.video_time_s - nearest.game_time_s));
}
