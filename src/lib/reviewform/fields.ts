/**
 * The review form's text fields, parsed.
 *
 * Kept out of the component so the rules are tested without a DOM: what a
 * clear time looks like, and what an empty box means. An empty box is always
 * "not entered" (`null`), never zero; for deaths that is what lets the form
 * fall back to the death markers.
 */

/** A parse that either produced a value or refused the text. */
export type Parsed<T> = { ok: true; value: T } | { ok: false };

const CLOCK = /^(\d{1,2}):([0-5]\d)$/;

/** "2:58" for 178 000 ms, "" for nothing entered. */
export function formatClock(ms: number | null): string {
  if (ms === null) return "";
  const total = Math.round(ms / 1000);
  return `${Math.floor(total / 60)}:${String(total % 60).padStart(2, "0")}`;
}

/** "m:ss" or "mm:ss" to milliseconds. Blank is `null`; anything else is refused. */
export function parseClock(text: string): Parsed<number | null> {
  const trimmed = text.trim();
  if (trimmed === "") return { ok: true, value: null };
  const match = CLOCK.exec(trimmed);
  if (!match) return { ok: false };
  return { ok: true, value: (Number(match[1]) * 60 + Number(match[2])) * 1000 };
}

/** A whole number of zero or more. Blank is `null`; anything else is refused. */
export function parseCount(text: string): Parsed<number | null> {
  const trimmed = text.trim();
  if (trimmed === "") return { ok: true, value: null };
  if (!/^\d+$/.test(trimmed)) return { ok: false };
  return { ok: true, value: Number(trimmed) };
}
