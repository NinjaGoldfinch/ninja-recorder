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

// Thresholds in seconds, with the unit each one switches to. Ordered
// coarsest-last; the first that fits wins.
const RELATIVE_STEPS: [limit: number, unit: Intl.RelativeTimeFormatUnit, per: number][] = [
  [60, "second", 1],
  [3600, "minute", 60],
  [86_400, "hour", 3600],
  [7 * 86_400, "day", 86_400],
];

/**
 * "16 minutes ago", for a row that is scanned rather than read.
 *
 * Past a week it hands back to `formatDateTime`: "6 weeks ago" is worse
 * than a date at that distance, because nobody counts weeks and the
 * absolute date is what a person would search their memory by.
 *
 * `Intl.RelativeTimeFormat` rather than a hand-rolled table, so the
 * plural rules and the wording follow the user's locale instead of
 * English's.
 */
export function formatRelative(millis: number): string {
  const seconds = (Date.now() - millis) / 1000;
  // A clock that disagrees with the file's timestamp should not produce
  // "in 3 minutes" on a recording that plainly already exists.
  if (seconds < 0) return formatDateTime(millis);

  const step = RELATIVE_STEPS.find(([limit]) => seconds < limit);
  if (!step) return formatDateTime(millis);

  const [, unit, per] = step;
  return new Intl.RelativeTimeFormat(undefined, { numeric: "auto" }).format(
    -Math.floor(seconds / per),
    unit,
  );
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
/**
 * `"15.3.412.9873"` → `"15.3"`.
 *
 * The whole string is what the column stores, because the build number is
 * what distinguishes two recordings made either side of a hotfix. It is
 * not what anyone calls a patch, though, so a row shows the two-part form
 * and keeps the rest for the hover.
 */
export function patchLabel(patch: string | null): string | null {
  if (patch === null) return null;
  const parts = patch.split(".");
  if (parts.length < 2) return patch;
  return `${parts[0]}.${parts[1]}`;
}

export function vodTitle(row: RecordingRow): string {
  return row.champion ?? gameModeLabel(row.game_mode) ?? basename(row.path);
}

/**
 * The long form: `"Viego vs Darius — Win"`.
 *
 * For the review view's heading and the row's accessible name, where there is
 * room for the thing that actually identifies a game. `vodTitle` stays the
 * short form, because the row's champion cell is a column and a matchup would
 * not fit in it.
 *
 * **Each half is added only when it is known**, so this degrades through
 * `"Viego vs Darius"`, `"Viego — Win"` and `"Viego"` rather than emitting
 * `"Viego vs undefined"`. The opponent comes from the caller, which is the
 * only place that has parsed the scoreboard.
 *
 * The filename can reach this through `vodTitle`, so callers still escape it.
 */
export function vodHeading(
  row: RecordingRow,
  opponent: string | null = null,
): string {
  const base = opponent === null ? vodTitle(row) : `${vodTitle(row)} vs ${opponent}`;
  // An undecided game says nothing rather than guessing, exactly as the row's
  // own outcome word does — a heading is the last place to imply a result.
  if (row.win === null) return base;
  return `${base} — ${row.win ? "Win" : "Loss"}`;
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

/**
 * `"Emerald III"`, or `"Master"` where divisions do not exist.
 *
 * Title case rather than the client's shouting — `EMERALD` is how the API
 * spells it and not how anybody says it. Absent when the game had no ladder:
 * a normal game has no rank, which is different from a rank we failed to read
 * and different again from being unranked, and the row does not pretend to
 * tell those apart because the column cannot.
 */
export function rankLabel(tier: string | null, division: string | null): string | null {
  if (tier === null || tier.trim() === "") return null;
  const name = tier.trim();
  const pretty = name.charAt(0).toUpperCase() + name.slice(1).toLowerCase();
  return division === null || division.trim() === "" ? pretty : `${pretty} ${division.trim()}`;
}

/** `"38 LP"`. Zero is a real standing, so it is shown rather than hidden. */
export function lpLabel(lp: number | null): string | null {
  return lp === null ? null : `${lp} LP`;
}
