/**
 * Timed notes (WS9 P1, #258): what each kind looks like, and where a note
 * sits in the recording.
 *
 * A note is stored in game time (`Note.ts_ms`), because it belongs to the game
 * and outlives the recording. The player works in video time, so every note
 * is placed once, here, through the same clock mapping that stamps it
 * (`review/clock.ts`), and everything that draws notes reads the placed form.
 */

import type { MarkerRow, SampleRow } from "../../types";
import type { Note, NoteKind } from "../contract/types";
import { videoAt } from "./clock";

export interface NoteStyle {
  label: string;
  icon: string;
  /** A token, so a kind follows the theme. */
  color: string;
}

/** In the order the kind picker lists them: the neutral default first. */
export const NOTE_KINDS: readonly NoteKind[] = ["note", "mistake", "good", "question", "takeaway"];

const NOTE_STYLE: Record<NoteKind, NoteStyle> = {
  note: { label: "Note", icon: "✎", color: "var(--text-muted)" },
  mistake: { label: "Mistake", icon: "✗", color: "var(--loss-fg)" },
  good: { label: "Good", icon: "✓", color: "var(--win-fg)" },
  question: { label: "Question", icon: "?", color: "var(--warn-fg)" },
  takeaway: { label: "Takeaway", icon: "★", color: "var(--accent)" },
};

export function noteStyle(kind: NoteKind): NoteStyle {
  return NOTE_STYLE[kind];
}

/** A note with the recording position it plays at. */
export interface PlacedNote {
  note: Note;
  videoTimeS: number;
}

/**
 * Every note at its position in the recording, in playback order. Ties keep
 * the order the notes were made in.
 */
export function placeNotes(
  notes: readonly Note[],
  recordingOffsetMs: number | null,
  markers: readonly MarkerRow[],
  samples: readonly SampleRow[],
): PlacedNote[] {
  return notes
    .map((note) => ({
      note,
      videoTimeS: videoAt(note.ts_ms / 1000, recordingOffsetMs, markers, samples),
    }))
    .sort((a, b) => a.videoTimeS - b.videoTimeS || a.note.created_at - b.note.created_at);
}
