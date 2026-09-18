/**
 * Keeping an isolated audio stem aligned with the video it plays over.
 *
 * Extracted from `review.ts`'s `correctStemDrift` by WS4.2. The decision and
 * the two element writes it drives are separated here, so the thresholds can
 * be tested without an `<audio>` element and a playing video.
 *
 * Track 0 is the combined mix and plays from the `<video>` itself. Any other
 * track is extracted to a sidecar and played through a hidden `<audio>`
 * against a muted video, because WebView2 offers no way to switch tracks
 * within one element (DEVELOPMENT.md 2.5). Two independent media elements
 * drift, so something has to pull them back together.
 */

/**
 * Below this, leave it alone.
 *
 * 0.04s is about 2.4 frames at 60fps, under the point A/V desync is
 * noticeable.
 */
export const SYNC_NUDGE = 0.04;

/**
 * Above this, seek rather than nudge.
 *
 * A rate nudge beyond roughly 2% is audible as a pitch shift, so a drift this
 * size cannot be eased away: it has to be cut, and a cut is audible as a
 * click, which is the lesser of the two.
 */
export const SYNC_HARD = 0.25;

export interface StemCorrection {
  /**
   * `seek` also sets the rate, because a seek lands the stem exactly where
   * the video is and any easing rate left over would immediately pull it off
   * again.
   */
  seek: boolean;
  playbackRate: number;
}

/**
 * What to do about a stem that is `driftS` seconds away from the video.
 *
 * Positive drift means the stem is ahead. Easing is asymmetric on purpose:
 * the stem is slowed to 98% when ahead and sped to 102% when behind, which
 * closes a sub-threshold gap over a second or two without a seek.
 */
export function stemCorrection(driftS: number, videoRate: number): StemCorrection {
  const drift = Math.abs(driftS);
  if (drift > SYNC_HARD) return { seek: true, playbackRate: videoRate };
  if (drift > SYNC_NUDGE) {
    return { seek: false, playbackRate: videoRate * (driftS > 0 ? 0.98 : 1.02) };
  }
  return { seek: false, playbackRate: videoRate };
}
