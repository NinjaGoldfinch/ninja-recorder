import { assetUrl, call } from "./bridge";
import { el, escapeAttr, escapeHtml } from "./dom";
import {
  formatBytes,
  formatClock,
  formatDateTime,
  formatKda,
  formatRelative,
  patchLabel,
  formatSpan,
  kdaRatio,
  queueOrModeLabel,
  vodTitle,
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
  // Rows are focusable, so they need to be openable from the keyboard —
  // they carried `tabindex` before the first redesign but no key handler.
  els.grid.addEventListener("keydown", (e) => {
    if (e.key !== "Enter" && e.key !== " ") return;
    const card = (e.target as HTMLElement).closest<HTMLElement>(".vod-row");
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
    if (championFilter && !vodTitle(row).toLowerCase().includes(championFilter)) {
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
  void fillInPortraits(rows);
}

/**
 * Champion portraits, resolved after the grid is already on screen.
 *
 * Deliberately a second pass rather than part of `card`. The first one is a
 * cache miss per champion and each miss is a CDN round trip, so blocking the
 * grid on it would trade a library that renders instantly for one that
 * renders once the network says so — and the whole thing has to work with no
 * network at all, where the answer is "no icon" and the card is already
 * correct without one.
 *
 * Cached per champion for the session, `null` included: a champion the CDN
 * has never heard of must not be asked about once per card, per render.
 */
const portraits = new Map<string, string | null>();

async function fillInPortraits(rows: RecordingRow[]) {
  const wanted = [...new Set(rows.map((r) => r.champion).filter((c): c is string => !!c))];
  const unknown = wanted.filter((c) => !portraits.has(c));

  await Promise.all(
    unknown.map(async (champion) => {
      try {
        const path = await call<string | null>("champion_icon", { champion });
        portraits.set(champion, path ? assetUrl(path) : null);
      } catch {
        // Offline, or the command is unavailable. Same outcome as an
        // unknown champion, and just as unremarkable.
        portraits.set(champion, null);
      }
    }),
  );

  for (const row of rows) {
    const src = row.champion ? portraits.get(row.champion) : null;
    if (!src) continue;
    // The grid may have been re-rendered while the requests were in flight.
    const slot = els.grid.querySelector<HTMLElement>(
      `.vod-row[data-id="${row.id}"] .vod-portrait`,
    );
    if (slot) slot.innerHTML = `<img src="${escapeAttr(src)}" alt="" loading="lazy" />`;
  }
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
  // `vodTitle` can fall back to the filename, which is user-controlled:
  // reconcile imports whatever video files it finds. Hence escapeAttr on
  // every attribute and escapeHtml on every text node below.
  const title = vodTitle(row);
  const kda = formatKda(row.kda_k, row.kda_d, row.kda_a);
  const queue = queueOrModeLabel(row);
  const length = row.duration_s === null ? null : formatClock(row.duration_s);

  const ratio = kdaRatio(row.kda_k, row.kda_d, row.kda_a);
  const patch = patchLabel(row.patch);

  // Said once, in the left block, and shown once, as the leading accent.
  // The word is what makes the row readable without colour — green and red
  // are exactly the pair a red-green deficiency cannot separate — and it
  // costs no column because that block is already a run of lines.
  //
  // Absent when the result is unknown, which is unambiguous rather than a
  // gap: a word is on every decided row, so no word means undecided.
  const outcomeWord = row.win === null ? null : row.win ? "Win" : "Loss";
  const outcome = outcomeWord ?? "Result unknown";

  // Every one of these can be absent, and a row that hides the slot when
  // it is reads as a different shape per recording — which is exactly what
  // makes a list scannable or not. `—` keeps the lines where they are and
  // says so out loud.
  const cell = (value: string | null, title?: string | null) =>
    value === null
      ? `<span class="vod-missing">—</span>`
      : `<span${title ? ` title="${escapeAttr(title)}"` : ""}>${escapeHtml(value)}</span>`;

  // Deaths in their own colour, which is the one number on a row people
  // look for first. Built from the three integers rather than by splitting
  // `formatKda`'s string, so nothing here has to parse its own output —
  // but `formatKda` still owns the all-three-or-nothing rule.
  const kdaMarkup =
    kda === null
      ? `<span class="vod-missing">—</span>`
      : `${row.kda_k} <span class="vod-slash">/</span>` +
        ` <span class="vod-deaths">${row.kda_d}</span>` +
        ` <span class="vod-slash">/</span> ${row.kda_a}`;

  return `
    <article class="vod-row" role="listitem" tabindex="0"
             data-id="${row.id}" data-outcome="${outcomeAttr(row.win)}"
             aria-label="${escapeAttr(`${title} — ${outcome}`)}">
      <span class="vod-meta">
        <span class="vod-queue">${cell(queue)}</span>
        <time datetime="${new Date(row.started_at).toISOString()}"
              class="vod-sub" title="${escapeAttr(formatDateTime(row.started_at))}"
        >${escapeHtml(formatRelative(row.started_at))}</time>
        <span class="vod-sub">${patch === null ? "&nbsp;" : `Patch ${escapeHtml(patch)}`}</span>
        <span class="vod-sub">${cell(length)}${
          outcomeWord ? ` · <span class="vod-outcome">${outcomeWord}</span>` : ""
        }</span>
      </span>

      <span class="vod-portrait" aria-hidden="true"></span>

      <span class="vod-cell">
        <span class="vod-champ" title="${escapeAttr(title)}">${escapeHtml(title)}</span>
        <span class="vod-sub">${row.role === null ? "&nbsp;" : escapeHtml(row.role)}</span>
      </span>

      <span class="vod-cell">
        <span class="vod-value vod-kda">${kdaMarkup}</span>
        <span class="vod-sub">${ratio ? escapeHtml(ratio) : "&nbsp;"}</span>
      </span>

      <span class="vod-slack" aria-hidden="true"></span>

      <span class="vod-sub vod-size">${formatBytes(row.size_bytes)}</span>

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

  const card = target.closest<HTMLElement>(".vod-row");
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
    toast(`Deleted ${row ? vodTitle(row) : "recording"}.`);
    await Promise.all([refreshLibrary(), refreshDiskUsage()]);
  } catch (err) {
    toast(`Failed to delete: ${err}`, "error");
  }
}
