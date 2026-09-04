// Pure display formatters. No DOM, no state.

export const BYTES_PER_GB = 1024 ** 3;
const BYTES_PER_MB = 1024 ** 2;

export function formatBytes(bytes: number): string {
  if (bytes >= BYTES_PER_GB) return `${(bytes / BYTES_PER_GB).toFixed(1)} GB`;
  return `${Math.round(bytes / BYTES_PER_MB)} MB`;
}

// `m:ss` — for positions within a VOD, where hours never come up.
export function formatTime(seconds: number): string {
  const m = Math.floor(seconds / 60);
  const s = Math.floor(seconds % 60);
  return `${m}:${s.toString().padStart(2, "0")}`;
}

// A game's length, as a game length is normally said: "32:14".
export function formatClock(seconds: number): string {
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  const s = Math.floor(seconds % 60);
  if (h > 0) {
    return `${h}:${m.toString().padStart(2, "0")}:${s
      .toString()
      .padStart(2, "0")}`;
  }
  return `${m}:${s.toString().padStart(2, "0")}`;
}

// Cumulative time across many games, where "1372:41" would be useless.
export function formatSpan(seconds: number): string {
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  if (h === 0) return `${m}m`;
  return `${h}h ${m}m`;
}

export function formatDateTime(millis: number): string {
  return new Date(millis).toLocaleString(undefined, {
    day: "numeric",
    month: "short",
    hour: "2-digit",
    minute: "2-digit",
  });
}

// Riot's queue ids. Only the ones this app is plausibly going to see —
// anything else falls back to the raw id rather than guessing, since a
// wrong queue name is worse than an unfamiliar number.
const QUEUE_NAMES: Record<number, string> = {
  0: "Custom",
  400: "Normal Draft",
  420: "Ranked Solo",
  430: "Normal Blind",
  440: "Ranked Flex",
  450: "ARAM",
  490: "Quickplay",
  700: "Clash",
  1700: "Arena",
  1900: "URF",
};

export function queueLabel(queue: number | null): string | null {
  if (queue === null) return null;
  return QUEUE_NAMES[queue] ?? `Queue ${queue}`;
}

export function basename(path: string): string {
  return path.split(/[\\/]/).pop() ?? path;
}
