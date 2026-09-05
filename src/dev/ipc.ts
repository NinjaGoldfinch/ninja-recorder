/**
 * Every backend call the portal makes goes through here, so the Log panel
 * has a complete record without each panel remembering to report itself.
 *
 * The app has no logging infrastructure — the Rust side writes `println!`
 * to whatever terminal is running `tauri dev`, and the frontend writes
 * nothing at all — so this is the only place a failed `invoke` is
 * durably visible.
 */
import { invoke } from "@tauri-apps/api/core";

export interface LogEntry {
  id: number;
  at: number;
  command: string;
  args: unknown;
  ok: boolean;
  ms: number;
  result?: unknown;
  error?: string;
}

/** Ring buffer. Long replays make thousands of calls; the log is a
 *  diagnostic, not an archive. */
const MAX_ENTRIES = 500;
const entries: LogEntry[] = [];
const listeners = new Set<(entries: readonly LogEntry[]) => void>();
let nextId = 1;

/**
 * Commands the portal polls on a timer. They are logged like everything
 * else but excluded from the Log panel by default — at 1 Hz they would
 * bury every call a human actually made within seconds.
 */
export const POLLED_COMMANDS = new Set(["dev_health", "dev_replay_status"]);

export function onLog(fn: (entries: readonly LogEntry[]) => void): () => void {
  listeners.add(fn);
  fn(entries);
  return () => listeners.delete(fn);
}

export function logEntries(): readonly LogEntry[] {
  return entries;
}

export function clearLog() {
  entries.length = 0;
  emit();
}

function emit() {
  for (const fn of listeners) fn(entries);
}

function push(entry: LogEntry) {
  entries.unshift(entry);
  if (entries.length > MAX_ENTRIES) entries.length = MAX_ENTRIES;
  emit();
}

/** `invoke`, plus timing and a log entry. Errors still throw. */
export async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  const started = performance.now();
  try {
    const result = await invoke<T>(command, args);
    push({
      id: nextId++,
      at: Date.now(),
      command,
      args,
      ok: true,
      ms: performance.now() - started,
      result,
    });
    return result;
  } catch (err) {
    push({
      id: nextId++,
      at: Date.now(),
      command,
      args,
      ok: false,
      ms: performance.now() - started,
      error: String(err),
    });
    throw err;
  }
}

/**
 * `call`, but returning the failure instead of throwing. Panels that
 * render several independent readouts use this so one unavailable
 * subsystem (no League running, say) doesn't blank the whole page.
 */
export async function tryCall<T>(
  command: string,
  args?: Record<string, unknown>,
): Promise<{ ok: true; value: T } | { ok: false; error: string }> {
  try {
    return { ok: true, value: await call<T>(command, args) };
  } catch (err) {
    return { ok: false, error: String(err) };
  }
}
