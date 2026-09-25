/**
 * What a recording lost to a capture failure, in words (#10).
 *
 * The daemon decides what counts as a failure (`recorder::problem`, and
 * `own::problem` for which audio sources are failures and which are only not
 * there) and sends the list twice: once as the `captureProblems` event, which
 * the strip above the views shows, and once inside the finished row's
 * `diagnostics_json`, which the library row and the review page show
 * afterwards. This module is the words for both. The desktop notification is
 * the daemon's own (`recorder::problem::notification`).
 *
 * **Every reason is untrusted text.** It is what a Windows call said, with its
 * HRESULT, and it is only ever interpolated by Svelte as text. Nothing here
 * builds markup, and nothing that renders these uses `{@html}`.
 */

import type { CaptureProblem, Event } from "../contract/types";

/** The event the strip is shown from. */
export type CaptureProblemsEvent = Extract<Event, { type: "captureProblems" }>;

/** What the strip says, and how loudly. */
export interface CaptureNotice {
  kind: "warn" | "error";
  text: string;
}

/** What a stored recording says it was recorded without. */
export interface RecordedWithout {
  /** For the library row: `Recorded without game audio`. */
  short: string;
  /** With every reason: for the row's tooltip and the review page. */
  full: string;
}

const KINDS = new Set(["sourceFailed", "sourceEnded", "endedEarly", "notStarted", "notSaved"]);

/**
 * How a source is named to a person, as `recorder::problem::source_label`
 * names it: `game audio`, `microphone audio`, `desktop audio`, and an
 * application by its executable without `.exe` (`Discord audio`).
 */
export function sourceLabel(source: string): string {
  if (source === "game" || source === "microphone" || source === "desktop") {
    return `${source} audio`;
  }
  const stem = source.toLowerCase().endsWith(".exe") ? source.slice(0, -4) : source;
  return `${stem} audio`;
}

/** What the recording is without, as a phrase that follows "without". */
export function lossLabel(problem: CaptureProblem): string {
  switch (problem.kind) {
    case "sourceFailed":
      return sourceLabel(problem.source);
    case "sourceEnded":
      return `part of the ${sourceLabel(problem.source)}`;
    case "endedEarly":
      return "the end of the game";
    case "notStarted":
    case "notSaved":
      return "the whole game";
  }
}

/** A reason without the full stop it may already end in. */
function trimStop(reason: string): string {
  return reason.trim().replace(/\.+$/, "");
}

/** `a`, `a and b`, `a, b and c`. */
function joinAnd(items: string[]): string {
  if (items.length <= 1) return items.join("");
  return `${items.slice(0, -1).join(", ")} and ${items[items.length - 1]}`;
}

function withReasons(problems: CaptureProblem[]): string {
  return problems.map((p) => `${lossLabel(p)} (${trimStop(p.reason)})`).join("; ");
}

function buildPhrase(build: number | null): string {
  return build === null ? "an unknown Windows build" : `Windows build ${build}`;
}

/**
 * The strip's text for one `captureProblems` event, or `null` when it carries
 * nothing to say.
 *
 * A game that was not recorded at all is an error; one that was saved with a
 * part missing is a warning. Either way it ends by asking for a report with
 * the Windows build, because the failures this carries are the ones that
 * should not happen and are only fixable with that report.
 */
export function noticeFor(event: CaptureProblemsEvent): CaptureNotice | null {
  const problems = event.problems.filter((p) => KINDS.has(p.kind));
  if (problems.length === 0) return null;
  const report =
    ` If this keeps happening, please report it with your Windows version ` +
    `(${buildPhrase(event.windowsBuild)}).`;
  const whole = problems.find((p) => p.kind === "notStarted" || p.kind === "notSaved");
  if (whole) {
    const lead =
      whole.kind === "notStarted"
        ? "This game was not recorded"
        : "The last recording could not be saved";
    return { kind: "error", text: `${lead}: ${trimStop(whole.reason)}.${report}` };
  }
  return {
    kind: "warn",
    text: `The last recording was saved without ${withReasons(problems)}.${report}`,
  };
}

/** A value that is a well-formed `CaptureProblem`, from JSON nobody vouched for. */
function isProblem(value: unknown): value is CaptureProblem {
  if (typeof value !== "object" || value === null) return false;
  const v = value as Record<string, unknown>;
  if (typeof v.kind !== "string" || !KINDS.has(v.kind) || typeof v.reason !== "string") {
    return false;
  }
  return (v.kind !== "sourceFailed" && v.kind !== "sourceEnded") || typeof v.source === "string";
}

/**
 * The capture problems stored in a row's `diagnostics_json`, or none.
 *
 * Tolerant by design: a row from before #10 has no such field, a row a rescan
 * imported has no diagnostics at all, and a malformed blob is a row with
 * nothing to say rather than a library that fails to render.
 */
export function storedProblems(diagnosticsJson: string | null): CaptureProblem[] {
  if (!diagnosticsJson) return [];
  let parsed: unknown;
  try {
    parsed = JSON.parse(diagnosticsJson);
  } catch {
    return [];
  }
  if (typeof parsed !== "object" || parsed === null) return [];
  const list = (parsed as { capture_problems?: unknown }).capture_problems;
  return Array.isArray(list) ? list.filter(isProblem) : [];
}

/** The "Recorded without …" line for a stored recording, or `null`. */
export function recordedWithout(diagnosticsJson: string | null): RecordedWithout | null {
  const problems = storedProblems(diagnosticsJson);
  if (problems.length === 0) return null;
  return {
    short: `Recorded without ${joinAnd(problems.map(lossLabel))}`,
    full: `Recorded without ${withReasons(problems)}.`,
  };
}
