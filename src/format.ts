// Pure display formatters. No DOM, no state.

import type { RecordingRow } from "./types";

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
  900: "ARURF",
  1020: "One for All",
  1300: "Nexus Blitz",
  1700: "Arena",
  1710: "Arena",
  1900: "URF",
};

export function queueLabel(queue: number | null): string | null {
  if (queue === null) return null;
  return QUEUE_NAMES[queue] ?? `Queue ${queue}`;
}

// Live Client Data's `gameData.gameMode`. Only ever consulted when there's
// no queue id, which is the Practice Tool / custom / no-LCU case.
//
// `CLASSIC` maps to the *map*, not to a queue: the mode string genuinely
// cannot tell blind from draft from ranked, and naming one would be a
// guess in a slot the user reads as fact.
const GAME_MODE_NAMES: Record<string, string> = {
  CLASSIC: "Summoner's Rift",
  ARAM: "ARAM",
  PRACTICETOOL: "Practice Tool",
  TUTORIAL: "Tutorial",
  URF: "URF",
  ONEFORALL: "One for All",
  NEXUSBLITZ: "Nexus Blitz",
  ULTBOOK: "Ultimate Spellbook",
  CHERRY: "Arena",
  STRAWBERRY: "Swarm",
};

export function gameModeLabel(mode: string | null): string | null {
  if (mode === null) return null;
  const key = mode.trim().toUpperCase();
  if (!key) return null;
  // An unfamiliar mode shows as-is rather than as nothing — same call as
  // `Queue 1234` above.
  return GAME_MODE_NAMES[key] ?? key;
}

// What the card's Queue slot shows. The two columns come from different
// sources and a row can carry either, both or neither: `queue` is Riot's
// real queue id and only the LCU can fill it, `game_mode` is captured live.
export function queueOrModeLabel(row: RecordingRow): string | null {
  return queueLabel(row.queue) ?? gameModeLabel(row.game_mode);
}

// The heading on a VOD card. Champion first, then the mode, then the
// filename — never empty, and never a guess dressed up as a fact.
//
// The filename is user-controlled (`reconcile` imports whatever video
// files it finds), so callers still have to escape the result.
export function vodTitle(row: RecordingRow): string {
  return row.champion ?? gameModeLabel(row.game_mode) ?? basename(row.path);
}

// "7 / 2 / 5". All three or nothing: a partial KDA reads as a real one.
export function formatKda(
  kills: number | null,
  deaths: number | null,
  assists: number | null,
): string | null {
  if (kills === null || deaths === null || assists === null) return null;
  return `${kills} / ${deaths} / ${assists}`;
}

// The ratio, as a tooltip rather than a fourth number in a card column
// three numbers wide. A deathless game has no finite ratio, so it is named
// instead of divided by zero.
export function kdaRatio(
  kills: number | null,
  deaths: number | null,
  assists: number | null,
): string | null {
  if (kills === null || deaths === null || assists === null) return null;
  if (deaths === 0) return kills + assists === 0 ? null : "Perfect KDA";
  return `${((kills + assists) / deaths).toFixed(2)} KDA`;
}

export function basename(path: string): string {
  return path.split(/[\\/]/).pop() ?? path;
}
