/**
 * The review session - WS4 task 4.5.
 *
 * What is held here is the *recording*: which one is open, its markers, its
 * samples, and the window they imply. What is deliberately **not** held here
 * is the player: `currentTime`, `paused`, `volume`, the selected track and the
 * fullscreen state all live on the elements that own them, because a
 * `<video>`'s position is not state anything should be diffing. See
 * `Review.svelte`, which is the imperative island this feeds.
 */

import { call } from "../../bridge";
import type { MarkerRow, RecordingRow, SampleRow } from "../../types";
import type { MetricKey } from "../timeline/graph";
import { measureGameEnd, measureGameStart, viewingWindow } from "../timeline/window";

let recording = $state<RecordingRow | null>(null);
let markers = $state<MarkerRow[]>([]);
let samples = $state<SampleRow[]>([]);
let metric = $state<MetricKey>("gold_diff");

/**
 * The file's duration, published by the component once `loadedmetadata`
 * fires.
 *
 * It lives here rather than being read off the element on demand because the
 * window depends on it and the window is what the timeline draws against. `0`
 * means "not known yet", which `viewingWindow` turns into an empty window
 * rather than a `NaN` one.
 */
let duration = $state(0);

export const review = {
  get recording() {
    return recording;
  },
  get markers() {
    return markers;
  },
  get samples() {
    return samples;
  },
  get metric() {
    return metric;
  },
  set metric(next: MetricKey) {
    metric = next;
  },
  get duration() {
    return duration;
  },
  /** Where the game sits inside the file. See `lib/timeline/window.ts`. */
  get window() {
    return viewingWindow(measureGameStart(samples), measureGameEnd(samples), duration);
  },
  get isOpen() {
    return recording !== null;
  },
};

export function setDuration(seconds: number) {
  duration = Number.isFinite(seconds) ? seconds : 0;
}

/**
 * Opens a recording, and loads its timeline.
 *
 * The row is set first and the markers awaited after, so the player starts
 * loading the file while the two queries are in flight. A failure costs the
 * timeline and not the video: the recording is the thing the user asked for.
 */
export async function openRecording(row: RecordingRow): Promise<void> {
  recording = row;
  markers = [];
  samples = [];
  duration = 0;

  try {
    const [loadedMarkers, loadedSamples] = await Promise.all([
      call<MarkerRow[]>("get_recording_markers", { recordingId: row.id }),
      call<SampleRow[]>("get_recording_samples", { recordingId: row.id }),
    ]);
    // Discarded if the user has already moved on: two recordings opened in
    // quick succession would otherwise race, and the slower query would win.
    if (recording?.id !== row.id) return;
    markers = loadedMarkers;
    samples = loadedSamples;
  } catch (err) {
    console.error("Failed to load timeline data", err);
  }
}

export function closeRecording() {
  recording = null;
  markers = [];
  samples = [];
  duration = 0;
}
