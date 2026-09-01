import { invoke } from "@tauri-apps/api/core";

let startBtn: HTMLButtonElement | null;
let stopBtn: HTMLButtonElement | null;
let refreshBtn: HTMLButtonElement | null;
let statusMsg: HTMLElement | null;
let libraryEmpty: HTMLElement | null;
let libraryList: HTMLUListElement | null;
let lcuCheckBtn: HTMLButtonElement | null;
let lcuStatusMsg: HTMLElement | null;
let gameStateBtn: HTMLButtonElement | null;
let gameStateMsg: HTMLElement | null;

interface LcuStatus {
  connected: boolean;
  phase: string | null;
  summoner: string | null;
  error: string | null;
}

interface SessionMarker {
  kind: string;
  game_time_s: number;
  video_time_s: number;
  payload: unknown;
}

interface FinalizedRecording {
  path: string;
  markers: SessionMarker[];
}

type GameState = "Idle" | "ClientRunning" | "WaitingForGame" | "Recording" | "Finalizing";

interface SupervisorStatus {
  state: GameState;
  last_finalized: FinalizedRecording | null;
}

function setStatus(text: string) {
  if (statusMsg) statusMsg.textContent = text;
}

async function refreshLibrary() {
  try {
    const names = await invoke<string[]>("list_recordings");
    if (!libraryList || !libraryEmpty) return;

    if (names.length === 0) {
      libraryEmpty.hidden = false;
      libraryList.hidden = true;
      libraryList.innerHTML = "";
      return;
    }

    libraryEmpty.hidden = true;
    libraryList.hidden = false;
    libraryList.innerHTML = names
      .map((name) => `<li>${escapeHtml(name)}</li>`)
      .join("");
  } catch (err) {
    setStatus(`Failed to list recordings: ${err}`);
  }
}

async function startRecording() {
  try {
    setStatus("Starting…");
    await invoke("start_recording");
    setStatus("Recording (stub).");
    if (startBtn) startBtn.disabled = true;
    if (stopBtn) stopBtn.disabled = false;
  } catch (err) {
    setStatus(`Failed to start: ${err}`);
  }
}

async function stopRecording() {
  try {
    setStatus("Stopping…");
    const path = await invoke<string>("stop_recording");
    setStatus(`Saved: ${path}`);
    if (startBtn) startBtn.disabled = false;
    if (stopBtn) stopBtn.disabled = true;
    await refreshLibrary();
  } catch (err) {
    setStatus(`Failed to stop: ${err}`);
  }
}

async function checkLcuStatus() {
  if (!lcuStatusMsg) return;
  lcuStatusMsg.textContent = "Checking…";
  try {
    const status = await invoke<LcuStatus>("lcu_status");
    if (status.error) {
      lcuStatusMsg.textContent = `Error: ${status.error}`;
    } else if (!status.connected) {
      lcuStatusMsg.textContent =
        "League Client not running (no lockfile found).";
    } else {
      lcuStatusMsg.textContent = `Connected. Summoner: ${status.summoner ?? "?"}. Phase: ${status.phase ?? "?"}.`;
    }
  } catch (err) {
    lcuStatusMsg.textContent = `Failed to check: ${err}`;
  }
}

async function checkGameState() {
  if (!gameStateMsg) return;
  gameStateMsg.textContent = "Checking…";
  try {
    const status = await invoke<SupervisorStatus>("game_state_status");
    let text = `State: ${status.state}.`;
    if (status.last_finalized) {
      text += ` Last recording: ${status.last_finalized.path} (${status.last_finalized.markers.length} markers).`;
    } else {
      text += " No recording finalized yet.";
    }
    gameStateMsg.textContent = text;
  } catch (err) {
    gameStateMsg.textContent = `Failed to check: ${err}`;
  }
}

function escapeHtml(value: string): string {
  const div = document.createElement("div");
  div.textContent = value;
  return div.innerHTML;
}

window.addEventListener("DOMContentLoaded", () => {
  startBtn = document.querySelector("#start-btn");
  stopBtn = document.querySelector("#stop-btn");
  refreshBtn = document.querySelector("#refresh-btn");
  statusMsg = document.querySelector("#status-msg");
  libraryEmpty = document.querySelector("#library-empty");
  libraryList = document.querySelector("#library-list");
  lcuCheckBtn = document.querySelector("#lcu-check-btn");
  lcuStatusMsg = document.querySelector("#lcu-status-msg");
  gameStateBtn = document.querySelector("#game-state-btn");
  gameStateMsg = document.querySelector("#game-state-msg");

  startBtn?.addEventListener("click", startRecording);
  stopBtn?.addEventListener("click", stopRecording);
  refreshBtn?.addEventListener("click", refreshLibrary);
  lcuCheckBtn?.addEventListener("click", checkLcuStatus);
  gameStateBtn?.addEventListener("click", checkGameState);

  refreshLibrary();
});
