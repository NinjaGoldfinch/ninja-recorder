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

/**
 * The point whose offset applies at `at`: the latest one at or before it,
 * or the earliest when `at` is before them all. Both directions below use it
 * (in video time one way, game time the other), so they pick the same point
 * and a note reads back where it was made. "Nearest" would not: two points
 * either side of a pause can be nearest in one clock and not the other.
 */
function governing(
  at: number,
  clock: (p: Clocked) => number,
  sources: readonly (readonly Clocked[])[],
): Clocked | null {
  let before: Clocked | null = null;
  let first: Clocked | null = null;
  for (const source of sources) {
    for (const point of source) {
      if (first === null || clock(point) < clock(first)) first = point;
      if (clock(point) <= at && (before === null || clock(point) > clock(before))) before = point;
    }
  }
  return before ?? first;
}

/**
 * Where a game time is in the recording: the other direction, for timed
 * notes (#258), which are stored in game time because a note belongs to the
 * game and outlives its recording.
 *
 * With nothing clocked, the game's stored `recording_offset_ms` (the game
 * clock at the recording's first frame) still places it. With neither, the
 * recording is one nothing ever clocked, and its notes were made in video
 * time, so they read back as video time.
 */
export function videoAt(
  gameTimeS: number,
  recordingOffsetMs: number | null,
  ...sources: (readonly Clocked[])[]
): number {
  const point = governing(gameTimeS, (p) => p.game_time_s, sources);
  if (point !== null) return Math.max(0, gameTimeS + (point.video_time_s - point.game_time_s));
  if (recordingOffsetMs !== null) return Math.max(0, gameTimeS - recordingOffsetMs / 1000);
  return Math.max(0, gameTimeS);
}

/**
 * The game time to stamp a note made at a position in the recording. The
 * inverse of `videoAt`, point for point, so a note made here reads back at the
 * same moment.
 */
export function noteTimeAt(
  videoTimeS: number,
  recordingOffsetMs: number | null,
  ...sources: (readonly Clocked[])[]
): number {
  const point = governing(videoTimeS, (p) => p.video_time_s, sources);
  if (point !== null) return Math.max(0, videoTimeS - (point.video_time_s - point.game_time_s));
  if (recordingOffsetMs !== null) return Math.max(0, videoTimeS + recordingOffsetMs / 1000);
  return Math.max(0, videoTimeS);
}
