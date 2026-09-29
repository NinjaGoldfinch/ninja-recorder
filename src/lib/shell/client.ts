/**
 * What the app bar's client pill and its hover card say.
 *
 * Pure: `status.svelte.ts` polls, `stores/client.svelte.ts` holds the raw
 * answers, and this turns them into words. The raw values are the LCU's own
 * spellings (`ChampSelect`, `WaitingForStats`) and are never shown as they
 * are; a phase this does not know is split into words rather than printed.
 *
 * **The recorder beats the client.** While a game is being recorded or saved
 * the pill says so, whatever phase the client reports, because that is the
 * thing this app is for.
 */

import { formatRelative, formatTime } from "../../format";
import type { GameState, LcuStatus, RecordingRow } from "../../types";

/** The pill's colour, which the stylesheet reads from `data-tone`. */
export type ClientTone =
  | "checking"
  | "closed"
  | "online"
  | "active"
  | "recording"
  | "saving"
  | "error";

/** What sits at the pill's left: the League glyph, a dot, or a spinner. */
export type ClientGlyph = "league" | "dot" | "spin";

export interface ClientPill {
  tone: ClientTone;
  glyph: ClientGlyph;
  label: string;
  /** Shown after the label: the name on short phases, the clock while
   *  recording, and nothing where the label is already long. */
  detail: string | null;
}

interface PhaseCopy {
  /** The pill's label. */
  pill: string;
  /** The hover card's "Phase" row, which has room to say more. */
  card: string;
  tone: ClientTone;
  glyph: ClientGlyph;
  /** Whether the pill has room for the name after the label. */
  showName: boolean;
}

const PHASES: Record<string, PhaseCopy> = {
  None: { pill: "Home", card: "In the client", tone: "online", glyph: "league", showName: true },
  Lobby: { pill: "In lobby", card: "In a lobby", tone: "online", glyph: "league", showName: true },
  Matchmaking: {
    pill: "In queue",
    card: "Searching for a match",
    tone: "active",
    glyph: "league",
    showName: true,
  },
  CheckedIntoTournament: {
    pill: "Checked in",
    card: "Checked into a tournament",
    tone: "active",
    glyph: "league",
    showName: false,
  },
  ReadyCheck: {
    pill: "Match found",
    card: "Accept or decline",
    tone: "active",
    glyph: "league",
    showName: false,
  },
  ChampSelect: {
    pill: "Champion select",
    card: "Champion select",
    tone: "active",
    glyph: "league",
    showName: false,
  },
  GameStart: {
    pill: "Loading in",
    card: "Loading screen",
    tone: "active",
    glyph: "spin",
    showName: false,
  },
  FailedToLaunch: {
    pill: "Game didn't launch",
    card: "The game failed to launch",
    tone: "error",
    glyph: "dot",
    showName: false,
  },
  InProgress: {
    pill: "In game",
    card: "In game",
    tone: "active",
    glyph: "league",
    showName: false,
  },
  Reconnect: {
    pill: "Reconnecting",
    card: "Reconnect available",
    tone: "active",
    glyph: "league",
    showName: false,
  },
  WaitingForStats: {
    pill: "Post-game",
    card: "Waiting for stats",
    tone: "online",
    glyph: "league",
    showName: true,
  },
  PreEndOfGame: {
    pill: "Post-game",
    card: "Honor and progression",
    tone: "online",
    glyph: "league",
    showName: true,
  },
  EndOfGame: {
    pill: "Post-game",
    card: "Post-game lobby",
    tone: "online",
    glyph: "league",
    showName: true,
  },
  TerminatedInError: {
    pill: "Game ended in error",
    card: "The game ended in an error",
    tone: "error",
    glyph: "dot",
    showName: false,
  },
};

/**
 * The phase as the backend spells it, unwrapped.
 *
 * The daemon formats `GameflowPhase` with `{:?}`, so a phase Rust has no
 * variant for arrives as `Unknown("Something")`. The quotes and the wrapper
 * are Rust's, not the client's, and are dropped here.
 */
export function rawPhase(phase: string | null): string | null {
  if (phase === null) return null;
  const unknown = /^Unknown\("(.*)"\)$/.exec(phase);
  return unknown ? unknown[1] : phase;
}

/** `TerminatedInError` → `Terminated in error`. */
export function splitWords(value: string): string {
  const words = value
    .replace(/([a-z0-9])([A-Z])/g, "$1 $2")
    .replace(/([A-Z]+)([A-Z][a-z])/g, "$1 $2")
    .replace(/[_-]+/g, " ")
    .trim()
    .toLowerCase();
  return words.charAt(0).toUpperCase() + words.slice(1);
}

function phaseCopy(phase: string | null): PhaseCopy {
  const raw = rawPhase(phase);
  if (raw === null) {
    return { pill: "Connected", card: "Unknown", tone: "online", glyph: "league", showName: true };
  }
  const known = PHASES[raw];
  if (known) return known;
  const words = splitWords(raw) || "Unknown";
  return { pill: "Connected", card: words, tone: "online", glyph: "league", showName: true };
}

/** The card's "Phase" row. */
export function phaseLabel(phase: string | null): string {
  return phaseCopy(phase).card;
}

/** A Riot ID split at its tag, so the tag can be drawn quieter. */
export function splitRiotId(summoner: string | null): { name: string; tag: string | null } | null {
  // The LCU sends an empty string, not null, before it knows who we are.
  if (!summoner) return null;
  const at = summoner.lastIndexOf("#");
  if (at <= 0) return { name: summoner, tag: null };
  return { name: summoner.slice(0, at), tag: summoner.slice(at + 1) || null };
}

export function clientPill(
  lcu: LcuStatus | null,
  game: GameState | null,
  elapsedS: number | null,
  failed = false,
): ClientPill {
  if (failed) return { tone: "error", glyph: "dot", label: "Status unavailable", detail: null };
  if (game === "Recording") {
    return {
      tone: "recording",
      glyph: "dot",
      label: "Recording",
      detail: elapsedS === null ? null : formatTime(elapsedS),
    };
  }
  if (game === "Finalizing") {
    return { tone: "saving", glyph: "spin", label: "Saving recording…", detail: null };
  }
  if (lcu === null) {
    return { tone: "checking", glyph: "spin", label: "Checking client…", detail: null };
  }
  if (lcu.error) return { tone: "error", glyph: "dot", label: "Can't reach client", detail: null };
  if (!lcu.connected) return { tone: "closed", glyph: "dot", label: "League closed", detail: null };

  const copy = phaseCopy(lcu.phase);
  // The client is in game and the recorder has not started yet: that is the
  // loading screen, whatever the phase is called at this instant.
  if (game === "WaitingForGame" && rawPhase(lcu.phase) === "InProgress") {
    return { tone: "active", glyph: "spin", label: "Loading in", detail: null };
  }
  const name = copy.showName ? (splitRiotId(lcu.summoner)?.name ?? null) : null;
  return { tone: copy.tone, glyph: copy.glyph, label: copy.pill, detail: name };
}

/** The steps of the card's progress track. */
export const PHASE_STEPS = ["Lobby", "Queue", "Select", "In game", "Post"] as const;

const STEP_OF: Record<string, number> = {
  Lobby: 0,
  Matchmaking: 1,
  CheckedIntoTournament: 1,
  ReadyCheck: 1,
  ChampSelect: 2,
  GameStart: 3,
  InProgress: 3,
  Reconnect: 3,
  WaitingForStats: 4,
  PreEndOfGame: 4,
  EndOfGame: 4,
};

/**
 * Which step of the track is lit, or `null` to leave the track out: in the
 * client's home screen, or in a phase that is not on the way to a game.
 */
export function phaseStep(lcu: LcuStatus | null): number | null {
  if (!lcu?.connected) return null;
  const raw = rawPhase(lcu.phase);
  return raw === null ? null : (STEP_OF[raw] ?? null);
}

export type RecorderTone = "ok" | "busy" | "recording" | "warn";

/**
 * The card's "Recorder" row: **Ready**, unless there is something to say.
 *
 * `daemon` is the pipe's state (`stores/daemon.svelte.ts`); anything but
 * `connected` means this window cannot see the recorder at all. `problem`
 * and `software` are `settings/capture.ts`'s `refusalNote` and
 * `softwareNote`, passed in already decided so the rule lives in one place.
 */
export function recorderLine(
  game: GameState | null,
  elapsedS: number | null,
  daemon: string | undefined,
  capture: { problem: string | null; software: string | null } | null,
): { tone: RecorderTone; text: string; title: string | null } {
  if (daemon === "reconnecting") {
    return { tone: "warn", text: "Not running", title: "Reconnecting to the recorder" };
  }
  if (daemon === "skewed") {
    return { tone: "warn", text: "Restart needed", title: "This window and the recorder differ" };
  }
  if (game === "Recording") {
    const clock = elapsedS === null ? "" : ` · ${formatTime(elapsedS)}`;
    return { tone: "recording", text: `Recording${clock}`, title: null };
  }
  if (game === "Finalizing") return { tone: "busy", text: "Saving…", title: null };
  if (capture?.problem) return { tone: "warn", text: "Can't record", title: capture.problem };
  if (capture?.software)
    return { tone: "warn", text: "Software encoding", title: capture.software };
  return { tone: "ok", text: "Ready", title: null };
}

/** The card's "Last saved" row: champion, result and when, never a path. */
export function lastSavedLine(rows: readonly RecordingRow[]): string | null {
  let newest: RecordingRow | null = null;
  for (const row of rows) {
    if (newest === null || row.started_at > newest.started_at) newest = row;
  }
  if (newest === null) return null;
  const parts = [newest.champion ?? "Recording"];
  if (newest.win !== null) parts.push(newest.win ? "Win" : "Loss");
  parts.push(formatRelative(newest.started_at));
  return parts.join(" · ");
}
