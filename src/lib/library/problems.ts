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
 * builds markup, and nothing that renders these uses `{@html}`. When the
 * daemon recognised the failure it also sends `explained`, plain words and a
 * fix, which are said instead; the reason stays in the stored diagnostics.
 */

import type { CaptureProblem, Event, Explained } from "../contract/types";

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

/** The explanation the daemon attached, if the problem kind carries one. */
function explainedOf(problem: CaptureProblem): Explained | undefined {
  return "explained" in problem ? problem.explained : undefined;
}

/**
 * The reason as a person is told it: the daemon's plain words when it
 * recognised the failure (#296), the technical reason when it did not.
 */
export function toldReason(problem: CaptureProblem): string {
  return explainedOf(problem)?.text ?? problem.reason;
}

/**
 * Whether a problem is worth a bug report: anything the daemon did not
 * explain as the user's or the machine's own doing, a device unplugged or a
 * privacy setting.
 */
export function wantsReport(problem: CaptureProblem): boolean {
  return explainedOf(problem)?.report ?? true;
}

/** Every fix the problems carry, each once, as sentences after the notice. */
function fixes(problems: CaptureProblem[]): string {
  const each = problems.map((p) => explainedOf(p)?.fix).filter((f): f is string => !!f);
  return [...new Set(each)].map((f) => ` ${f}`).join("");
}

function withReasons(problems: CaptureProblem[]): string {
  return problems.map((p) => `${lossLabel(p)} (${trimStop(toldReason(p))})`).join("; ");
}

function buildPhrase(build: number | null): string {
  return build === null ? "an unknown Windows build" : `Windows build ${build}`;
}

/**
 * The strip's text for one `captureProblems` event, or `null` when it carries
 * nothing to say.
 *
 * A game that was not recorded at all is an error; one that was saved with a
 * part missing is a warning. It ends by asking for a report with the Windows
 * build when anything it carries should not happen and is only fixable with
 * that report. A failure the daemon recognised as the user's or the
 * machine's doing (a microphone unplugged, or blocked in Windows' privacy
 * settings) is said in plain words, with its fix, and asks for nothing (#296).
 */
export function noticeFor(event: CaptureProblemsEvent): CaptureNotice | null {
  const problems = event.problems.filter((p) => KINDS.has(p.kind));
  if (problems.length === 0) return null;
  const report = problems.some(wantsReport)
    ? ` If this keeps happening, please report it with your Windows version ` +
      `(${buildPhrase(event.windowsBuild)}).`
    : "";
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
    text: `The last recording was saved without ${withReasons(problems)}.${fixes(problems)}${report}`,
  };
}

/** A value that is a well-formed `Explained`, from JSON nobody vouched for. */
function isExplained(value: unknown): value is Explained {
  if (typeof value !== "object" || value === null) return false;
  const v = value as Record<string, unknown>;
  return (
    typeof v.text === "string" &&
    (v.fix === null || typeof v.fix === "string") &&
    typeof v.report === "boolean"
  );
}

/**
 * A value that is a well-formed `CaptureProblem`, from JSON nobody vouched
 * for. An `explained` that is there must be well-formed too: a malformed one
 * could otherwise drop the report request on a failure nobody explained.
 */
function isProblem(value: unknown): value is CaptureProblem {
  if (typeof value !== "object" || value === null) return false;
  const v = value as Record<string, unknown>;
  if (typeof v.kind !== "string" || !KINDS.has(v.kind) || typeof v.reason !== "string") {
    return false;
  }
  if (v.explained !== undefined && !isExplained(v.explained)) return false;
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
    full: `Recorded without ${withReasons(problems)}.${fixes(problems)}`,
  };
}
