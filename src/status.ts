import { call } from "./bridge";
import { el } from "./dom";
import { formatTime } from "./format";
import { refreshDiskUsage, refreshLibrary } from "./library";
import type { GameState, LcuStatus, SupervisorStatus } from "./types";

// There are no Tauri events on the Rust side — every backend→frontend
// signal is pull-only — so the header's live state comes from a poll.
//
// A setTimeout chain rather than setInterval: `lcu_status` reads a lockfile
// and makes two HTTPS round trips to the client, and a slow tick under
// setInterval would stack calls on top of each other. Chaining makes
// overlap structurally impossible instead of guarding against it.
const INTERVALS: Record<GameState, number> = {
  Recording: 1500,
  Finalizing: 1500,
  // Matches the Rust lockfile watcher's own 2s cadence; faster buys nothing.
  WaitingForGame: 2000,
  ClientRunning: 3000,
  Idle: 5000,
};
const HIDDEN_INTERVAL = 10000;

// `lcu_status` is the expensive call; `game_state_status` is a mutex read
// with no I/O. Poll the cheap one every tick and the costly one rarely,
// plus immediately whenever the state changes.
const LCU_EVERY = 4;

// Catches anything that changed without passing through the state machine
// — a retention sweep, or files moved in the folder behind our back.
const SAFETY_REFRESH_MS = 60_000;

interface Els {
  lcuPill: HTMLElement;
  lcuText: HTMLElement;
  gamePill: HTMLElement;
  gameText: HTMLElement;
  aboutLcu: HTMLElement;
  aboutState: HTMLElement;
  aboutFinalized: HTMLElement;
}

let els: Els;
let timer: number | undefined;
let stopped = false;

let lcuCountdown = 0;
let lastLcu: LcuStatus | null = null;
let prevState: GameState | null = null;
let prevFinalizedPath: string | null = null;
let recordingElapsed: number | null = null;
let lastSafetyRefresh = 0;

export function initStatus() {
  els = {
    lcuPill: el("#status-lcu"),
    lcuText: el("#status-lcu-text"),
    gamePill: el("#status-game"),
    gameText: el("#status-game-text"),
    aboutLcu: el("#about-lcu"),
    aboutState: el("#about-game-state"),
    aboutFinalized: el("#about-last-finalized"),
  };
  lastSafetyRefresh = performance.now();
  tick();
}

export function stopStatusPolling() {
  stopped = true;
  window.clearTimeout(timer);
}

async function tick() {
  if (stopped) return;
  let state: GameState = prevState ?? "Idle";
  try {
    state = await pollOnce();
  } catch (err) {
    renderError(err);
  }
  if (stopped) return;
  const delay = document.hidden ? HIDDEN_INTERVAL : INTERVALS[state];
  timer = window.setTimeout(tick, delay);
}

async function pollOnce(): Promise<GameState> {
  const status = await call<SupervisorStatus>("game_state_status");
  const changed = status.state !== prevState;

  if (changed || lcuCountdown <= 0) {
    lcuCountdown = LCU_EVERY;
    lastLcu = await call<LcuStatus>("lcu_status");
    renderLcu(lastLcu);
  }
  lcuCountdown -= 1;

  recordingElapsed = status.recording_elapsed_s;

  renderGame(status);

  // A finished game should appear on its own. Derived from the two edges
  // already in the payload rather than polling `list_recordings`, which
  // would rebuild the grid every couple of seconds and fight scroll
  // position and focus for no reason.
  const finalizedPath = status.last_finalized?.path ?? null;
  const justFinished = prevState === "Finalizing" && status.state !== "Finalizing";
  const newRecording =
    finalizedPath !== null && finalizedPath !== prevFinalizedPath && prevState !== null;

  prevState = status.state;
  prevFinalizedPath = finalizedPath;

  const now = performance.now();
  if (justFinished || newRecording || now - lastSafetyRefresh > SAFETY_REFRESH_MS) {
    lastSafetyRefresh = now;
    void refreshLibrary();
    void refreshDiskUsage();
  }

  return status.state;
}

function setPill(pill: HTMLElement, text: HTMLElement, state: string, copy: string) {
  pill.dataset.state = state;
  text.textContent = copy;
}

function renderLcu(status: LcuStatus) {
  if (status.error) {
    setPill(els.lcuPill, els.lcuText, "error", "Client error");
    els.aboutLcu.textContent = `Error: ${status.error}`;
    return;
  }
  if (!status.connected) {
    setPill(els.lcuPill, els.lcuText, "offline", "Client not running");
    els.aboutLcu.textContent = "Not running (no lockfile found).";
    return;
  }
  // The LCU hands back an empty string, not null, when it has no Riot ID
  // for us yet — so fall back on falsiness rather than nullishness.
  const who = status.summoner || "signed in";
  setPill(els.lcuPill, els.lcuText, "online", who);
  els.aboutLcu.textContent = `Connected as ${who} — phase ${status.phase ?? "?"}.`;
}

const GAME_COPY: Record<GameState, { state: string; copy: string }> = {
  Idle: { state: "idle", copy: "Idle" },
  ClientRunning: { state: "idle", copy: "Waiting for a game" },
  WaitingForGame: { state: "armed", copy: "Game starting…" },
  Recording: { state: "recording", copy: "Recording" },
  Finalizing: { state: "finalizing", copy: "Saving…" },
};

function renderGame(status: SupervisorStatus) {
  const { state, copy } = GAME_COPY[status.state];
  const elapsed =
    status.state === "Recording" && recordingElapsed !== null
      ? ` — ${formatTime(recordingElapsed)}`
      : "";
  setPill(els.gamePill, els.gameText, state, `${copy}${elapsed}`);

  els.aboutState.textContent = status.state;
  const finalized = status.last_finalized;
  if (!finalized) {
    els.aboutFinalized.textContent = "None yet.";
    return;
  }
  // A null recording_id means the file exists but its row never got
  // written — worth saying out loud rather than rendering as a blank.
  const idNote =
    finalized.recording_id === null
      ? "DB WRITE FAILED"
      : `db id ${finalized.recording_id}`;
  els.aboutFinalized.textContent = `${finalized.path} (${finalized.markers.length} markers, ${idNote})`;
}

function renderError(err: unknown) {
  setPill(els.gamePill, els.gameText, "error", "Status unavailable");
  els.aboutState.textContent = `Failed to read: ${err}`;
}

// Without this, every hot reload leaves its poll loop running and the
// League client gets hit by N concurrent status calls.
if (import.meta.hot) import.meta.hot.dispose(stopStatusPolling);
