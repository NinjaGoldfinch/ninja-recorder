import { call } from "../../bridge";
import type { GameState, LcuStatus, SupervisorStatus } from "../../types";
import { finalizedLine, gamePill, lcuLine, lcuPill } from "../settings/about";
import {
  setAboutGameState,
  setAboutLastFinalized,
  setAboutLcu,
  setGamePill,
  setLcuPill,
} from "./about.svelte";
import { refreshDiskUsage, refreshLibrary } from "./library.svelte";
import { refreshCaptureBackend } from "./settings.svelte";
import { refreshUpdateStatus } from "./update.svelte";

// The two Tauri events the backend pushes (`library-changed`,
// `update-status-changed`) are both once-in-a-while facts; nothing pushes the
// header's live state, so that comes from a poll.
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

let timer: number | undefined;
let stopped = false;

let lcuCountdown = 0;
let lastLcu: LcuStatus | null = null;
let prevState: GameState | null = null;
let prevFinalizedPath: string | null = null;
let recordingElapsed: number | null = null;
let lastSafetyRefresh = 0;

/// `document.hidden` is only read when the *next* delay is chosen, so without
/// this the header keeps its old cadence until the in-flight timer fires: up to
/// 10s of staleness on the way back, and one more full-rate poll on the way
/// out. Re-polling immediately on becoming visible fixes the first; the second
/// costs one tick and isn't worth cancelling a timer over.
function onVisibilityChange() {
  if (stopped || document.hidden) return;
  window.clearTimeout(timer);
  void tick();
}

export function initStatus() {
  document.addEventListener("visibilitychange", onVisibilityChange);
  lastSafetyRefresh = performance.now();
  tick();
}

export function stopStatusPolling() {
  stopped = true;
  window.clearTimeout(timer);
  document.removeEventListener("visibilitychange", onVisibilityChange);
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

  // Whether an offered update can be installed depends on this exact value,
  // and the update check that computed it last runs every six hours. Without
  // this the Install button would stay enabled through a whole game and only
  // refuse at the click. Only on the edge: `get_update_status` is a mutex
  // read, but so is this poll, and every tick would be waste.
  if (changed) void refreshUpdateStatus();

  // The same edge is when the capture backend's answer moves: the own backend
  // learns its encoder when the client opens, which is when Settings can
  // first say it is encoding in software.
  if (changed) void refreshCaptureBackend();

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
  // The safety sweep rebuilds the whole grid (`innerHTML`), so it is pure
  // waste against a window nobody can see. Skipping it while hidden also
  // leaves `lastSafetyRefresh` stale, which makes the first poll after the
  // window comes back catch up immediately. The two edges below still fire
  // while hidden — they are once-a-game, and they keep `library-changed`
  // honest if the event is ever missed.
  const safetyDue = !document.hidden && now - lastSafetyRefresh > SAFETY_REFRESH_MS;
  if (justFinished || newRecording || safetyDue) {
    lastSafetyRefresh = now;
    void refreshLibrary();
    void refreshDiskUsage();
  }

  return status.state;
}

// The wording lives in `lib/settings/about.ts` since WS4.4 so that it could be
// tested, and WS4.6 took the elements away too: both the About lines and the
// app bar's pills now go to a store. This module keeps the poll, which is the
// thing it was always for.
function renderLcu(status: LcuStatus) {
  setLcuPill(lcuPill(status));
  setAboutLcu(lcuLine(status));
}

function renderGame(status: SupervisorStatus) {
  setGamePill(gamePill(status.state, recordingElapsed));
  setAboutGameState(status.state);
  setAboutLastFinalized(finalizedLine(status));
}

function renderError(err: unknown) {
  setGamePill({ state: "error", copy: "Status unavailable" });
  setAboutGameState(`Failed to read: ${err}`);
}

// Without this, every hot reload leaves its poll loop running and the
// League client gets hit by N concurrent status calls.
if (import.meta.hot) import.meta.hot.dispose(stopStatusPolling);
