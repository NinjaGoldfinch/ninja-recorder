import { call } from "./bridge";
import { el } from "./dom";
import { finalizedLine, gamePill, lcuLine, lcuPill } from "./lib/settings/about";
import { setAboutGameState, setAboutLastFinalized, setAboutLcu } from "./lib/stores/about.svelte";
import { refreshDiskUsage, refreshLibrary } from "./lib/stores/library.svelte";
import { refreshUpdateStatus } from "./lib/stores/update.svelte";
import type { GameState, LcuStatus, SupervisorStatus } from "./types";

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

interface Els {
  lcuPill: HTMLElement;
  lcuText: HTMLElement;
  gamePill: HTMLElement;
  gameText: HTMLElement;
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
  els = {
    lcuPill: el("#status-lcu"),
    lcuText: el("#status-lcu-text"),
    gamePill: el("#status-game"),
    gameText: el("#status-game-text"),
  };
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

function setPill(pill: HTMLElement, text: HTMLElement, state: string, copy: string) {
  pill.dataset.state = state;
  text.textContent = copy;
}

// The wording moved to `lib/settings/about.ts` in WS4.4 so that it could be
// tested, and the About lines now go to a store rather than to elements this
// module used to reach into. The pills are still written here: they live in
// the app bar, which is vanilla markup until WS4.6.
function renderLcu(status: LcuStatus) {
  const pill = lcuPill(status);
  setPill(els.lcuPill, els.lcuText, pill.state, pill.copy);
  setAboutLcu(lcuLine(status));
}

function renderGame(status: SupervisorStatus) {
  const pill = gamePill(status.state, recordingElapsed);
  setPill(els.gamePill, els.gameText, pill.state, pill.copy);
  setAboutGameState(status.state);
  setAboutLastFinalized(finalizedLine(status));
}

function renderError(err: unknown) {
  setPill(els.gamePill, els.gameText, "error", "Status unavailable");
  setAboutGameState(`Failed to read: ${err}`);
}

// Without this, every hot reload leaves its poll loop running and the
// League client gets hit by N concurrent status calls.
if (import.meta.hot) import.meta.hot.dispose(stopStatusPolling);
