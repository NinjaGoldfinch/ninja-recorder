import { invoke } from "@tauri-apps/api/core";
import { initReview, openReview, type RecordingRow } from "./review";

let startBtn: HTMLButtonElement | null;
let stopBtn: HTMLButtonElement | null;
let refreshBtn: HTMLButtonElement | null;
let rescanBtn: HTMLButtonElement | null;
let statusMsg: HTMLElement | null;
let libraryEmpty: HTMLElement | null;
let libraryList: HTMLUListElement | null;
let lcuCheckBtn: HTMLButtonElement | null;
let lcuStatusMsg: HTMLElement | null;
let gameStateBtn: HTMLButtonElement | null;
let gameStateMsg: HTMLElement | null;
let filterChampion: HTMLInputElement | null;
let filterOutcome: HTMLSelectElement | null;
let filterPinned: HTMLInputElement | null;
let sortSelect: HTMLSelectElement | null;
let diskUsageText: HTMLElement | null;
let retentionForm: HTMLFormElement | null;
let retentionSizeEnabled: HTMLInputElement | null;
let retentionSizeGb: HTMLInputElement | null;
let retentionAgeEnabled: HTMLInputElement | null;
let retentionAgeDays: HTMLInputElement | null;
let retentionStatus: HTMLElement | null;

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
  recording_id: number | null;
  path: string;
  markers: SessionMarker[];
}

interface ReconcileReport {
  orphans_removed: number;
  imported: number;
}

interface DiskUsage {
  total_bytes: number;
  recording_count: number;
  free_bytes: number;
}

interface RetentionPolicy {
  max_total_bytes: number | null;
  max_age_days: number | null;
}

const BYTES_PER_GB = 1024 ** 3;

type GameState = "Idle" | "ClientRunning" | "WaitingForGame" | "Recording" | "Finalizing";

interface SupervisorStatus {
  state: GameState;
  last_finalized: FinalizedRecording | null;
}

// The full set fetched from the DB; filters/sort below operate on this
// in-memory rather than re-querying, since the dataset is small and local.
let allRecordings: RecordingRow[] = [];

function setStatus(text: string) {
  if (statusMsg) statusMsg.textContent = text;
}

function basename(path: string): string {
  return path.split(/[\\/]/).pop() ?? path;
}

function formatKda(row: RecordingRow): string {
  if (row.kda_k === null || row.kda_d === null || row.kda_a === null) {
    return "—/—/—";
  }
  return `${row.kda_k}/${row.kda_d}/${row.kda_a}`;
}

function formatOutcomeBadge(win: boolean | null): string {
  if (win === null) return "";
  return win
    ? `<span class="badge badge-win">Win</span>`
    : `<span class="badge badge-loss">Loss</span>`;
}

function formatBytes(bytes: number): string {
  if (bytes >= BYTES_PER_GB) return `${(bytes / BYTES_PER_GB).toFixed(1)} GB`;
  return `${Math.round(bytes / 1024 ** 2)} MB`;
}

function formatRecordingRow(row: RecordingRow): string {
  const when = new Date(row.started_at).toLocaleString();
  const pinned = `<button
      class="pin-btn${row.pinned ? " pinned" : ""}"
      type="button"
      data-pin="${row.id}"
      title="${row.pinned ? "Unpin" : "Pin (exempt from disk retention)"}"
    >📌</button>`;
  const champion = row.champion
    ? escapeHtml(row.champion)
    : escapeHtml(basename(row.path));
  const queue = row.queue !== null ? `<span>Queue ${row.queue}</span>` : "";

  return `
    <li data-id="${row.id}" tabindex="0">
      <div class="library-row-main">
        <span class="library-row-title">${champion}</span>
        ${formatOutcomeBadge(row.win)}
      </div>
      <div class="library-row-meta">
        <span>${formatKda(row)}</span>
        ${queue}
        <span class="hint">${when}</span>
        ${pinned}
      </div>
    </li>`;
}

function currentSort(): string {
  return sortSelect?.value ?? "newest";
}

function applyFiltersAndRender() {
  if (!libraryList || !libraryEmpty) return;

  const championFilter = (filterChampion?.value ?? "").trim().toLowerCase();
  const outcomeFilter = filterOutcome?.value ?? "all";
  const pinnedOnly = filterPinned?.checked ?? false;

  let rows = allRecordings.filter((row) => {
    if (championFilter && !(row.champion ?? "").toLowerCase().includes(championFilter)) {
      return false;
    }
    if (outcomeFilter === "wins" && row.win !== true) return false;
    if (outcomeFilter === "losses" && row.win !== false) return false;
    if (pinnedOnly && !row.pinned) return false;
    return true;
  });

  rows = rows.slice().sort((a, b) => {
    switch (currentSort()) {
      case "oldest":
        return a.started_at - b.started_at;
      case "longest":
        return (b.duration_s ?? 0) - (a.duration_s ?? 0);
      case "champion":
        return (a.champion ?? "").localeCompare(b.champion ?? "");
      case "newest":
      default:
        return b.started_at - a.started_at;
    }
  });

  if (rows.length === 0) {
    libraryEmpty.hidden = false;
    libraryList.hidden = true;
    libraryList.innerHTML = "";
    return;
  }

  libraryEmpty.hidden = true;
  libraryList.hidden = false;
  libraryList.innerHTML = rows.map(formatRecordingRow).join("");
}

async function refreshLibrary() {
  try {
    allRecordings = await invoke<RecordingRow[]>("list_recordings");
    applyFiltersAndRender();
  } catch (err) {
    setStatus(`Failed to list recordings: ${err}`);
  }
}

async function rescanRecordings() {
  try {
    setStatus("Rescanning…");
    const report = await invoke<ReconcileReport>("rescan_recordings");
    setStatus(
      `Rescan complete: removed ${report.orphans_removed} orphan row(s), imported ${report.imported} untracked file(s).`,
    );
    await refreshLibrary();
  } catch (err) {
    setStatus(`Failed to rescan: ${err}`);
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
    await Promise.all([refreshLibrary(), refreshDiskUsage()]);
  } catch (err) {
    setStatus(`Failed to stop: ${err}`);
  }
}

async function refreshDiskUsage() {
  if (!diskUsageText) return;
  try {
    const usage = await invoke<DiskUsage>("get_disk_usage");
    diskUsageText.textContent =
      `${formatBytes(usage.total_bytes)} across ${usage.recording_count} recording(s) — ` +
      `${formatBytes(usage.free_bytes)} free`;
  } catch (err) {
    diskUsageText.textContent = `Failed to load disk usage: ${err}`;
  }
}

function applyRetentionPolicyToForm(policy: RetentionPolicy) {
  if (!retentionSizeEnabled || !retentionSizeGb || !retentionAgeEnabled || !retentionAgeDays) return;

  retentionSizeEnabled.checked = policy.max_total_bytes !== null;
  retentionSizeGb.disabled = policy.max_total_bytes === null;
  retentionSizeGb.value =
    policy.max_total_bytes !== null ? String(Math.round(policy.max_total_bytes / BYTES_PER_GB)) : "";

  retentionAgeEnabled.checked = policy.max_age_days !== null;
  retentionAgeDays.disabled = policy.max_age_days === null;
  retentionAgeDays.value = policy.max_age_days !== null ? String(policy.max_age_days) : "";
}

async function loadRetentionPolicy() {
  try {
    const policy = await invoke<RetentionPolicy>("get_retention_policy");
    applyRetentionPolicyToForm(policy);
  } catch (err) {
    if (retentionStatus) retentionStatus.textContent = `Failed to load policy: ${err}`;
  }
}

async function saveRetentionPolicy(e: Event) {
  e.preventDefault();
  if (!retentionSizeEnabled || !retentionSizeGb || !retentionAgeEnabled || !retentionAgeDays || !retentionStatus) {
    return;
  }

  const policy: RetentionPolicy = {
    max_total_bytes: retentionSizeEnabled.checked
      ? Math.round(Number(retentionSizeGb.value) * BYTES_PER_GB)
      : null,
    max_age_days: retentionAgeEnabled.checked ? Number(retentionAgeDays.value) : null,
  };

  try {
    retentionStatus.textContent = "Saving…";
    await invoke("set_retention_policy", { policy });
    retentionStatus.textContent = "Saved.";
    await Promise.all([refreshDiskUsage(), refreshLibrary()]);
  } catch (err) {
    retentionStatus.textContent = `Failed to save: ${err}`;
  }
}

async function togglePin(row: RecordingRow) {
  try {
    await invoke("set_pinned", { recordingId: row.id, pinned: !row.pinned });
    await refreshLibrary();
  } catch (err) {
    setStatus(`Failed to update pin: ${err}`);
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
      const idNote =
        status.last_finalized.recording_id !== null
          ? `db id ${status.last_finalized.recording_id}`
          : "DB WRITE FAILED";
      text += ` Last recording: ${status.last_finalized.path} (${status.last_finalized.markers.length} markers, ${idNote}).`;
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
  rescanBtn = document.querySelector("#rescan-btn");
  filterChampion = document.querySelector("#filter-champion");
  filterOutcome = document.querySelector("#filter-outcome");
  filterPinned = document.querySelector("#filter-pinned");
  sortSelect = document.querySelector("#sort-select");
  diskUsageText = document.querySelector("#disk-usage-text");
  retentionForm = document.querySelector("#retention-form");
  retentionSizeEnabled = document.querySelector("#retention-size-enabled");
  retentionSizeGb = document.querySelector("#retention-size-gb");
  retentionAgeEnabled = document.querySelector("#retention-age-enabled");
  retentionAgeDays = document.querySelector("#retention-age-days");
  retentionStatus = document.querySelector("#retention-status");

  startBtn?.addEventListener("click", startRecording);
  stopBtn?.addEventListener("click", stopRecording);
  refreshBtn?.addEventListener("click", refreshLibrary);
  rescanBtn?.addEventListener("click", rescanRecordings);
  lcuCheckBtn?.addEventListener("click", checkLcuStatus);
  gameStateBtn?.addEventListener("click", checkGameState);

  filterChampion?.addEventListener("input", applyFiltersAndRender);
  filterOutcome?.addEventListener("change", applyFiltersAndRender);
  filterPinned?.addEventListener("change", applyFiltersAndRender);
  sortSelect?.addEventListener("change", applyFiltersAndRender);

  libraryList?.addEventListener("click", (e) => {
    const pinBtn = (e.target as HTMLElement).closest<HTMLButtonElement>("button[data-pin]");
    if (pinBtn) {
      const row = allRecordings.find((r) => r.id === Number(pinBtn.dataset.pin));
      if (row) togglePin(row);
      return;
    }

    const li = (e.target as HTMLElement).closest<HTMLLIElement>("li[data-id]");
    if (!li) return;
    const row = allRecordings.find((r) => r.id === Number(li.dataset.id));
    if (row) openReview(row);
  });

  retentionSizeEnabled?.addEventListener("change", () => {
    if (retentionSizeGb) retentionSizeGb.disabled = !retentionSizeEnabled?.checked;
  });
  retentionAgeEnabled?.addEventListener("change", () => {
    if (retentionAgeDays) retentionAgeDays.disabled = !retentionAgeEnabled?.checked;
  });
  retentionForm?.addEventListener("submit", saveRetentionPolicy);

  initReview();
  refreshLibrary();
  refreshDiskUsage();
  loadRetentionPolicy();
});
