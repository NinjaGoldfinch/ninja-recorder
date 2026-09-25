/**
 * The dev portal's formatters - #72.
 *
 * Moved out of `src/dev/ui.ts`, which was a module of markup builders with
 * these four mixed in. They were always pure; what they lacked was a test,
 * because `vitest.config.ts` excluded `src/dev/**` on the grounds that the
 * portal was staying vanilla and out of scope. It is not, so they did not.
 *
 * Deliberately separate from `src/format.ts`. The app's formatters answer to a
 * product decision about what a person reading a library wants to see; these
 * answer to a developer reading a debug panel, and the two should be free to
 * disagree. `bytes` is the clearest case: this one says `1.50 GB` where the
 * app's `formatBytes` rounds differently, because a number being diagnosed is
 * not a number being skimmed.
 */

/** A missing value, said the same way everywhere. */
export const MISSING = "—";

export function bytes(n: number | null | undefined): string {
  if (n === null || n === undefined) return MISSING;
  const gb = 1024 ** 3;
  const mb = 1024 ** 2;
  if (Math.abs(n) >= gb) return `${(n / gb).toFixed(2)} GB`;
  if (Math.abs(n) >= mb) return `${(n / mb).toFixed(1)} MB`;
  if (Math.abs(n) >= 1024) return `${(n / 1024).toFixed(0)} KB`;
  return `${n} B`;
}

/**
 * `m:ss`, clamped at zero.
 *
 * Negative is not an error worth reporting here: a duration derived from two
 * clocks can go slightly negative while a recording is starting, and `-0:01`
 * on a debug panel reads as a bug in the panel.
 */
export function duration(seconds: number | null | undefined): string {
  if (seconds === null || seconds === undefined) return MISSING;
  const total = Math.max(0, Math.round(seconds));
  const m = Math.floor(total / 60);
  const s = total % 60;
  return `${m}:${String(s).padStart(2, "0")}`;
}

export function timestamp(millis: number): string {
  return new Date(millis).toLocaleString();
}

/** 24-hour, because a log line read beside a timestamped file should not
 *  need the reader to work out which half of the day it was. */
export function clockTime(millis: number): string {
  return new Date(millis).toLocaleTimeString(undefined, { hour12: false });
}

/**
 * The capture worker, said so that "this backend has none" cannot be read as
 * "it is down". The stub and a refusing backend have no worker; libobs and the
 * own backend spawn one while League runs and end it when it closes.
 */
export function workerState(running: boolean | null | undefined): string {
  if (running === undefined) return MISSING;
  if (running === null) return "none (this backend has no worker)";
  return running ? "up" : "not running";
}
