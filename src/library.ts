import { call } from "./bridge";
import { el, escapeAttr, escapeHtml } from "./dom";
import {
  basename,
  formatBytes,
  formatClock,
  formatDateTime,
  formatSpan,
  queueLabel,
} from "./format";
import { getPrefs } from "./prefs";
import { currentView, onViewChange } from "./router";
import { openReview } from "./review";
import { toast } from "./toast";
import type { DiskUsage, ReconcileReport, RecordingRow } from "./types";

interface Els {
  grid: HTMLElement;
  empty: HTMLElement;
  champion: HTMLInputElement;
  outcome: HTMLSelectElement;
  pinned: HTMLInputElement;
  sort: HTMLSelectElement;
  refresh: HTMLButtonElement;
  rescan: HTMLButtonElement;
  statGames: HTMLElement;
  statGamesSub: HTMLElement;
  statWinrate: HTMLElement;
  statWinrateSub: HTMLElement;
  statPlaytime: HTMLElement;
  statPlaytimeSub: HTMLElement;
  statDisk: HTMLElement;
  statDiskSub: HTMLElement;
}

let els: Els;

// The full set fetched from the DB; filters/sort below operate on this
// in-memory rather than re-querying, since the dataset is small and local.
let allRecordings: RecordingRow[] = [];
let usage: DiskUsage | null = null;

// A refresh that lands while the user is in the review or settings view
// writes the data but defers the re-render. Rebuilding the grid under a
// hidden view is wasted work, and doing it as they navigate back would
// yank the card they came from out from under them.
let pendingRender = false;

// Delete is a two-step on the button itself rather than a modal: the
// recording id currently armed, if any.
let armedForDelete: number | null = null;
let armTimer: number | undefined;

export function initLibrary() {
  els = {
    grid: el("#library-grid"),
    empty: el("#library-empty"),
    champion: el<HTMLInputElement>("#filter-champion"),
    outcome: el<HTMLSelectElement>("#filter-outcome"),
    pinned: el<HTMLInputElement>("#filter-pinned"),
    sort: el<HTMLSelectElement>("#sort-select"),
    refresh: el<HTMLButtonElement>("#refresh-btn"),
    rescan: el<HTMLButtonElement>("#rescan-btn"),
    statGames: el("#stat-games"),
    statGamesSub: el("#stat-games-sub"),
    statWinrate: el("#stat-winrate"),
    statWinrateSub: el("#stat-winrate-sub"),
    statPlaytime: el("#stat-playtime"),
    statPlaytimeSub: el("#stat-playtime-sub"),
    statDisk: el("#stat-disk"),
    statDiskSub: el("#stat-disk-sub"),
  };

  els.champion.addEventListener("input", render);
  els.outcome.addEventListener("change", render);
  els.pinned.addEventListener("change", render);
  els.sort.addEventListener("change", render);
  els.refresh.addEventListener("click", () => {
    refreshLibrary();
    refreshDiskUsage();
  });
  els.rescan.addEventListener("click", rescanRecordings);

  els.grid.addEventListener("click", onGridClick);
  // Cards are focusable, so they need to be openable from the keyboard —
  // they carried `tabindex` before this redesign but no key handler.
  els.grid.addEventListener("keydown", (e) => {
    if (e.key !== "Enter" && e.key !== " ") return;
    const card = (e.target as HTMLElement).closest<HTMLElement>(".vod-card");
    if (!card) return;
    e.preventDefault();
    const row = findRow(Number(card.dataset.id));
    if (row) openReview(row);
  });

  onViewChange((view) => {
    if (view === "library" && pendingRender) render();
  });
}

// Called once prefs have loaded, which is after the first render.
export function applyDefaultSort() {
  els.sort.value = getPrefs().defaultSort;
  render();
}

function findRow(id: number): RecordingRow | undefined {
  return allRecordings.find((r) => r.id === id);
}

export async function refreshLibrary() {
  try {
    allRecordings = await call<RecordingRow[]>("list_recordings");
    render();
  } catch (err) {
    toast(`Failed to list recordings: ${err}`, "error");
  }
}

export async function refreshDiskUsage() {
  try {
    usage = await call<DiskUsage>("get_disk_usage");
    renderStats(visibleRows());
  } catch (err) {
    toast(`Failed to load disk usage: ${err}`, "error");
  }
}

async function rescanRecordings() {
  try {
    els.rescan.disabled = true;
    const report = await call<ReconcileReport>("rescan_recordings");
    toast(
      `Rescan complete — removed ${report.orphans_removed} orphan row(s), imported ${report.imported} untracked file(s).`,
    );
    await Promise.all([refreshLibrary(), refreshDiskUsage()]);
  } catch (err) {
    toast(`Failed to rescan: ${err}`, "error");
  } finally {
    els.rescan.disabled = false;
  }
}

function visibleRows(): RecordingRow[] {
  const championFilter = els.champion.value.trim().toLowerCase();
  const outcome = els.outcome.value;
  const pinnedOnly = els.pinned.checked;

  const rows = allRecordings.filter((row) => {
    if (
      championFilter &&
      !(row.champion ?? basename(row.path)).toLowerCase().includes(championFilter)
    ) {
      return false;
    }
    if (outcome === "wins" && row.win !== true) return false;
    if (outcome === "losses" && row.win !== false) return false;
    if (pinnedOnly && !row.pinned) return false;
    return true;
  });

  return rows.sort((a, b) => {
    switch (els.sort.value) {
      case "oldest":
        return a.started_at - b.started_at;
      case "longest":
        return (b.duration_s ?? 0) - (a.duration_s ?? 0);
      case "champion":
        return (a.champion ?? "").localeCompare(b.champion ?? "");
      default:
        return b.started_at - a.started_at;
    }
  });
}

function render() {
  if (currentView() !== "library") {
    pendingRender = true;
    return;
  }
  pendingRender = false;

  const rows = visibleRows();
  renderStats(rows);

  disarmDelete();
  if (rows.length === 0) {
    els.empty.hidden = false;
    els.grid.hidden = true;
    els.grid.innerHTML = "";
    return;
  }

  els.empty.hidden = true;
  els.grid.hidden = false;
  els.grid.innerHTML = rows.map(card).join("");
}

// Stats are computed over the *filtered* rows so they track the filters,
// with a sub-label naming the total whenever a filter is narrowing things
// — otherwise "100%" under Wins-only reads as a perfect record.
function renderStats(rows: RecordingRow[]) {
  els.statGames.textContent = String(rows.length);
  els.statGamesSub.textContent =
    rows.length === allRecordings.length ? "" : `of ${allRecordings.length}`;

  // `win` is null for anything reconcile imported — it only knows the path
  // and size. Treating that as a loss would quietly understate the rate.
  const decided = rows.filter((r) => r.win !== null);
  const wins = decided.filter((r) => r.win === true).length;
  els.statWinrate.textContent =
    decided.length === 0 ? "—" : `${Math.round((wins / decided.length) * 100)}%`;
  els.statWinrateSub.textContent =
    decided.length === 0 ? "no results yet" : `${wins}W ${decided.length - wins}L`;

  // Same story for `duration_s`.
  const timed = rows.filter((r) => r.duration_s !== null);
  els.statPlaytime.textContent = formatSpan(
    timed.reduce((total, r) => total + (r.duration_s ?? 0), 0),
  );
  els.statPlaytimeSub.textContent =
    timed.length === rows.length ? "" : `${rows.length - timed.length} unknown`;

  // `size_bytes` is NOT NULL DEFAULT 0, so no null handling here.
  els.statDisk.textContent = formatBytes(
    rows.reduce((total, r) => total + r.size_bytes, 0),
  );
  els.statDiskSub.textContent = usage
    ? `${formatBytes(usage.free_bytes)} free`
    : "";
}

function outcomeAttr(win: boolean | null): string {
  if (win === null) return "unknown";
  return win ? "win" : "loss";
}

function card(row: RecordingRow): string {
  // Falls back to the filename, which is user-controlled: reconcile imports
  // whatever video files it finds. Hence escapeAttr on every attribute.
  const title = row.champion ?? basename(row.path);
  const kda =
    row.kda_k === null || row.kda_d === null || row.kda_a === null
      ? null
      : `${row.kda_k} / ${row.kda_d} / ${row.kda_a}`;
  const queue = queueLabel(row.queue);
  const length = row.duration_s === null ? null : formatClock(row.duration_s);

  const badge =
    row.win === null
      ? ""
      : `<span class="badge badge-${row.win ? "win" : "loss"}">${
          row.win ? "Win" : "Loss"
        }</span>`;

  const stat = (label: string, value: string | null) =>
    `<div><dt>${label}</dt><dd>${value === null ? "—" : escapeHtml(value)}</dd></div>`;

  return `
    <article class="vod-card" role="listitem" tabindex="0"
             data-id="${row.id}" data-outcome="${outcomeAttr(row.win)}">
      <header class="vod-card-head">
        <span class="vod-champ" title="${escapeAttr(title)}">${escapeHtml(title)}</span>
        ${badge}
      </header>
      <dl class="vod-stats">
        ${stat("KDA", kda)}
        ${stat("Length", length)}
        ${stat("Queue", queue)}
      </dl>
      <footer class="vod-card-foot">
        <time datetime="${new Date(row.started_at).toISOString()}">${escapeHtml(
          formatDateTime(row.started_at),
        )}</time>
        <span class="vod-size">${formatBytes(row.size_bytes)}</span>
        <span class="vod-actions">
          <button class="icon-btn pin-btn${row.pinned ? " pinned" : ""}"
                  type="button" data-pin="${row.id}"
                  aria-pressed="${row.pinned}"
                  title="${row.pinned ? "Unpin" : "Pin (exempt from disk retention)"}"
          >📌</button>
          <button class="icon-btn danger" type="button" data-delete="${row.id}"
                  aria-label="Delete recording" title="Delete recording"
          >🗑</button>
        </span>
      </footer>
    </article>`;
}

function onGridClick(e: MouseEvent) {
  const target = e.target as HTMLElement;

  const pinBtn = target.closest<HTMLButtonElement>("button[data-pin]");
  if (pinBtn) {
    const row = findRow(Number(pinBtn.dataset.pin));
    if (row) togglePin(row);
    return;
  }

  const deleteBtn = target.closest<HTMLButtonElement>("button[data-delete]");
  if (deleteBtn) {
    onDeleteClick(deleteBtn, Number(deleteBtn.dataset.delete));
    return;
  }

  // Anything else inside the actions group must not fall through to
  // opening the VOD.
  if (target.closest(".vod-actions")) return;

  const card = target.closest<HTMLElement>(".vod-card");
  if (!card) return;
  const row = findRow(Number(card.dataset.id));
  if (row) openReview(row);
}

async function togglePin(row: RecordingRow) {
  try {
    await call("set_pinned", { recordingId: row.id, pinned: !row.pinned });
    await refreshLibrary();
  } catch (err) {
    toast(`Failed to update pin: ${err}`, "error");
  }
}

// Two-step confirm in place of a dialog: the first click arms the button,
// the second deletes. Cheaper than a modal and it keeps the destructive
// action next to the thing it destroys.
function onDeleteClick(button: HTMLButtonElement, id: number) {
  if (armedForDelete === id) {
    disarmDelete();
    deleteRecording(id);
    return;
  }
  disarmDelete();
  armedForDelete = id;
  button.classList.add("armed");
  button.textContent = "Delete?";
  window.clearTimeout(armTimer);
  armTimer = window.setTimeout(disarmDelete, 4000);
}

function disarmDelete() {
  window.clearTimeout(armTimer);
  if (armedForDelete === null) return;
  const button = els.grid.querySelector<HTMLButtonElement>(
    `button[data-delete="${armedForDelete}"]`,
  );
  if (button) {
    button.classList.remove("armed");
    button.textContent = "🗑";
  }
  armedForDelete = null;
}

async function deleteRecording(id: number) {
  const row = findRow(id);
  try {
    await call("delete_recording", { recordingId: id });
    toast(`Deleted ${row?.champion ?? basename(row?.path ?? "recording")}.`);
    await Promise.all([refreshLibrary(), refreshDiskUsage()]);
  } catch (err) {
    toast(`Failed to delete: ${err}`, "error");
  }
}
