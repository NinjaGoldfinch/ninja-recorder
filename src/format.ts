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

export function basename(path: string): string {
  return path.split(/[\\/]/).pop() ?? path;
}
