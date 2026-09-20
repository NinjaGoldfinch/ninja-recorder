/**
 * What went wrong with a file, and what the recording claims is in it.
 *
 * Extracted from `review.ts` by WS4.5. The error text in particular is the
 * app's only diagnosis of a file it cannot play, and it was written inside a
 * function that assigned three elements.
 */

import type { AudioLayout } from "../../types";

/**
 * The per-recording track layout, as stored by the recorder.
 *
 * `null` for anything we did not record: a rescan-imported file, or a VOD made
 * before multi-track audio existed. **Parse failures are treated the same
 * way**, because an unreadable layout is an unknown one, and the picker hides
 * rather than guessing at a file's contents.
 */
export function parseAudioLayout(json: string | null): AudioLayout | null {
  if (!json) return null;
  try {
    const parsed = JSON.parse(json) as AudioLayout;
    return Array.isArray(parsed?.tracks) ? parsed : null;
  } catch {
    return null;
  }
}

/**
 * Whether the stem picker is worth showing.
 *
 * Fewer than two tracks is nothing to choose between, and so is a layout we
 * do not have.
 */
export function hasStems(layout: AudioLayout | null): boolean {
  return (layout?.tracks.length ?? 0) >= 2;
}

/**
 * `MediaError` codes in words.
 *
 * Surfaces the actual code rather than a canned guess: a decode error and an
 * unreachable file are different problems with different fixes, and the
 * wording of 3 and 4 says which half of "it didn't play" actually failed.
 */
const MEDIA_ERROR_LABELS: Record<number, string> = {
  1: "Aborted",
  2: "Network error",
  3: "Decode error \u2014 the container loaded but the codec inside it isn't supported",
  4: "Source not supported \u2014 wrong format, or the file couldn't be reached at all",
};

export interface VideoErrorReport {
  message: string;
  /** The technical half, for someone reporting it. */
  detail: string;
}

/**
 * Why a recording would not play, in the most specific terms available.
 *
 * Two special cases earn their length, because both are common and neither is
 * guessable from a generic message.
 */
export function videoErrorReport(
  code: number | null,
  errorMessage: string | null,
  src: string,
  path: string | null,
): VideoErrorReport {
  const isMkv = path?.toLowerCase().endsWith(".mkv") ?? false;
  // Codes 3 (decode) and 4 (source not supported) on an mp4 that is not
  // actually malformed are, in practice, almost always an unsupported codec
  // inside an otherwise-valid container.
  const likelyCodecIssue = !isMkv && (code === 3 || code === 4);

  let message: string;
  if (isMkv) {
    // High-confidence special case: WebView2's `<video>` has no Matroska
    // demuxer at all, so an .mkv fails regardless of how valid its contents
    // are. Most likely to bite anyone testing with an OBS recording, since
    // .mkv is OBS's crash-safe default output.
    message =
      "This is an .mkv file — browsers (including WebView2) can't play Matroska containers natively, no matter what's encoded inside. Remux it to .mp4 (e.g. \"ffmpeg -i in.mkv -c copy out.mp4\", no re-encode needed) and try again.";
  } else if (likelyCodecIssue) {
    // H.265/HEVC-in-mp4 is the single most common real-world cause: many
    // capture tools (ShadowPlay, some phones) default to it, and WebView2
    // cannot decode it without a Windows codec pack that is not installed by
    // default.
    message = `This recording's video couldn't be played (${MEDIA_ERROR_LABELS[code as number]}). The most common cause for an otherwise-valid mp4 is H.265/HEVC video — WebView2 needs the "HEVC Video Extensions" from the Microsoft Store to decode it at all, and playback can still be unreliable even then. Re-encoding to H.264 is the more reliable fix: "ffmpeg -i in.mp4 -c:v libx264 -c:a aac out.mp4".`;
  } else if (code !== null) {
    message = `This recording's video couldn't be played (${MEDIA_ERROR_LABELS[code] ?? `error code ${code}`}).`;
  } else {
    message = "This recording's video couldn't be played.";
  }

  const parts: string[] = [];
  if (errorMessage) parts.push(errorMessage);
  parts.push(`src: ${src}`);

  return { message, detail: parts.join(" — ") };
}
