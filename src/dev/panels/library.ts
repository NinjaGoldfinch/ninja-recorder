/**
 * One recording, from every source that knows something about it.
 *
 * The information already existed and was scattered — the Database panel has
 * the raw row, Diagnostics has what the finalize observed, the Log has what
 * the poller saw, the review view has the markers. This is the one place
 * they sit together, which is what is actually wanted when a row looks
 * wrong: a role that says Jungle for a top game, empty item slots, a missing
 * champion (#99).
 *
 * **Provenance is the point.** `champion` and `role` have two possible
 * writers each and a rule about which wins; `queue` comes from the gameflow
 * session. Listing values alone would leave the reader to remember all of
 * that, so every ambiguous field is shown with the thing that likely wrote
 * it — and `role`/`patch` are the certain ones, because only the deferred
 * patch writes them, which makes their absence evidence rather than a shrug.
 */
import type { Panel } from "../main";
import { tryCall } from "../ipc";
import type { Provenance, RecordingReport, RecordingRow } from "../types";
import { bytes, card, duration, escapeHtml, kv, output, panelHead, timestamp } from "../ui";

let root: HTMLElement | null = null;
let rows: RecordingRow[] = [];
let selected: number | null = null;
let report: RecordingReport | null = null;
let reportError: string | null = null;

function title(row: RecordingRow): string {
  return row.champion ?? row.path.split(/[\\/]/).pop() ?? `recording ${row.id}`;
}

function list(): string {
  if (rows.length === 0) {
    return card("Library", "<p class='hint'>No recordings. The Seed panel writes some.</p>");
  }
  const items = rows
    .map((row) => {
      const on = row.id === selected ? " selected" : "";
      return `<button type="button" class="list-row${on}" data-open="${row.id}">
        <span class="list-main">${escapeHtml(title(row))}</span>
        <span class="hint">${timestamp(row.started_at)} · ${duration(row.duration_s)} · ${bytes(row.size_bytes)}</span>
      </button>`;
    })
    .join("");
  return card("Library", `<div class="list">${items}</div>`);
}

/**
 * Named writers, worst-known-first is not the ordering — the backend returns
 * them in a fixed order so the table does not reshuffle between recordings.
 * A field nothing wrote shows as `—`, which is its own answer.
 */
function provenance(r: RecordingReport): string {
  const rowsHtml = r.provenance
    .map(
      (p: Provenance) => `<tr>
        <td><code>${escapeHtml(p.field)}</code></td>
        <td>${p.source ? escapeHtml(p.source) : "<span class='hint'>—</span>"}</td>
        <td class="hint">${escapeHtml(p.note)}</td>
      </tr>`,
    )
    .join("");
  return card(
    "Where each value came from",
    `<table class="kv-table"><thead><tr><th>Field</th><th>Likely writer</th><th>Why</th></tr></thead>
     <tbody>${rowsHtml}</tbody></table>
     <p class="hint">Derived from what the row looks like — nothing records a per-column writer,
     so this is a strong hint rather than an audit. <code>role</code> and <code>patch</code> are
     the certain ones: only the deferred patch writes them.</p>`,
  );
}

function counts(r: RecordingReport): string {
  // Gold is called out separately because it comes from a different source on
  // a different schedule. Plenty of samples and no gold is the #137
  // fingerprint — the live poller ran, the deferred patch never finished —
  // rather than a contradiction.
  const goldNote =
    r.samples > 0 && r.gold_samples === 0
      ? "<p class='hint'>Samples but no gold: the live poller ran and the deferred patch never landed. Settings → Storage → fill in can recover it while the client still remembers the game.</p>"
      : "";
  return card(
    "What it carries",
    kv([
      ["Markers", String(r.markers)],
      ["Samples", String(r.samples)],
      ["…with gold", String(r.gold_samples)],
      [
        "Alignment offset",
        r.alignment_offset_s === null ? null : `${r.alignment_offset_s.toFixed(2)}s`,
      ],
    ]) + goldNote,
  );
}

function detail(): string {
  if (selected === null) {
    return card("Recording", "<p class='hint'>Pick one on the left.</p>");
  }
  if (reportError !== null) {
    return card("Recording", output(reportError, true));
  }
  if (report === null) {
    return card("Recording", "<p class='hint'>Loading…</p>");
  }
  const r = report;
  return (
    card("Row", output(r.row)) +
    provenance(r) +
    counts(r) +
    card(
      "Scoreboard",
      r.scoreboard === null
        ? "<p class='hint'>None. A game whose poller never saw a player list, or a file <code>reconcile</code> imported.</p>"
        : output(r.scoreboard),
    ) +
    card(
      "Diagnostics",
      r.diagnostics === null
        ? "<p class='hint'>None. Anything recorded before migration 7, and anything <code>reconcile</code> imported.</p>"
        : output(r.diagnostics),
    )
  );
}

function paint() {
  if (!root) return;
  const target = root.querySelector<HTMLElement>("#library-body");
  if (target) target.innerHTML = `<div class="split">${list()}<div>${detail()}</div></div>`;
}

async function load() {
  const result = await tryCall<RecordingRow[]>("list_recordings");
  rows = result.ok ? result.value : [];
  paint();
}

async function open(id: number) {
  selected = id;
  report = null;
  reportError = null;
  paint();
  const result = await tryCall<RecordingReport>("dev_recording_report", {
    recordingId: id,
  });
  if (result.ok) {
    report = result.value;
  } else {
    // Shown rather than swallowed: "no recording 12" is the answer when a row
    // was deleted between listing it and opening it, and that is worth seeing.
    reportError = result.error;
  }
  paint();
}

export const libraryPanel: Panel = {
  id: "library",
  title: "Library",
  icon: "▤",
  group: "Tools",

  mount(el, ctx) {
    root = el;
    el.innerHTML =
      panelHead(
        "Library",
        "One recording from every source that knows something about it — the row, where each value came from, and what it carries.",
      ) +
      `<div class="row" style="margin-bottom:.8rem">
        <button type="button" class="ghost" data-reload>Reload</button>
      </div>
      <div id="library-body"></div>`;

    el.addEventListener("click", (e) => {
      const target = e.target as HTMLElement;
      if (target.closest("[data-reload]")) {
        void load();
        return;
      }
      const open_ = target.closest<HTMLElement>("[data-open]");
      if (open_) void open(Number(open_.dataset.open));
    });

    // Deep link from the main window's rows: `#/library/<id>` arrives as the
    // navigation payload, so the panel opens on the recording somebody was
    // already looking at rather than on an empty list.
    const wanted = Number(ctx.payload);
    void load().then(() => {
      if (Number.isFinite(wanted) && wanted > 0) void open(wanted);
    });
  },

  unmount() {
    root = null;
  },
};
