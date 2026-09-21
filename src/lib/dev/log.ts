/**
 * The two log views' filtering rules.
 *
 * The backend view filters server-side - the file is capped at 5 MiB, far too
 * much to hand a webview in one string, so `dev_read_log` does the matching
 * and returns a window. What is left here is which query to ask for. The
 * portal's own IPC list is small enough to filter in the page.
 */

import { type LogEntry, POLLED_COMMANDS } from "../../dev/ipc";

/** Newest first, and every level on, so an untouched panel shows the file. */
export const LEVELS = ["ERROR", "WARN", "INFO", "DEBUG"] as const;

/**
 * Tags hidden on first render.
 *
 * Named as tags to *hide* rather than tags to show, which is what lets them be
 * hidden before the panel has read the file and learned which tags exist. At
 * 1 Hz `live-poll` alone would bury a session's real errors.
 */
export const HIDDEN_TAGS = ["live-poll", "libobs"];

export function levelTone(level: string): "err" | "warn" | "" {
  if (level === "ERROR") return "err";
  if (level === "WARN") return "warn";
  return "";
}

/**
 * Toggling a level.
 *
 * **The last one cannot be turned off.** An empty level set means "no filter"
 * on the Rust side, so unticking everything would show the whole file rather
 * than nothing, which reads as a bug.
 */
export function toggleLevel(levels: readonly string[], level: string): string[] {
  if (!levels.includes(level)) return [...levels, level];
  if (levels.length === 1) return [...levels];
  return levels.filter((l) => l !== level);
}

export function toggleTag(hidden: readonly string[], tag: string): string[] {
  return hidden.includes(tag) ? hidden.filter((t) => t !== tag) : [...hidden, tag];
}

export interface IpcFilter {
  search: string;
  showPolled: boolean;
}

/**
 * The portal's own calls, filtered.
 *
 * Polled commands are hidden by default: at 1 Hz they bury everything a person
 * actually clicked within seconds.
 */
export function visibleEntries(
  entries: readonly LogEntry[],
  { search, showPolled }: IpcFilter,
): LogEntry[] {
  const needle = search.trim().toLowerCase();
  return entries.filter((e) => {
    if (!showPolled && POLLED_COMMANDS.has(e.command)) return false;
    if (!needle) return true;
    const haystack = `${e.command} ${JSON.stringify(e.args ?? "")} ${e.error ?? ""}`.toLowerCase();
    return haystack.includes(needle);
  });
}

/** The argument summary shown on a collapsed row. */
export function argSummary(args: unknown, max = 90): string {
  if (args === undefined || args === null) return "";
  const text = JSON.stringify(args);
  if (text === undefined) return "";
  return text.length > max ? `${text.slice(0, max)}…` : text;
}
