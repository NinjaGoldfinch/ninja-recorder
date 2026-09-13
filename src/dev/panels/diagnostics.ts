/**
 * What each recording's finalize actually observed, as against what the
 * recording contains.
 *
 * The library shows you a card. This shows you why the card says what it
 * says — or why it says nothing. `recordings.diagnostics_json` (migration
 * 7) is written at finalize because none of it survives the game: Live
 * Client Data is gone the moment it ends.
 *
 * The panel leads with what is **wrong or missing** rather than dumping
 * fields. A row of eleven numbers is not an answer; "we were never found
 * in allPlayers, which is why this card has no champion" is.
 */
import type { Panel } from "../main";
import { tryCall } from "../ipc";
import type { RecordingDiagnostics, RecordingRow } from "../types";
import { card, duration, escapeHtml, kv, output, panelHead, timestamp, toast } from "../ui";

let root: HTMLElement | null = null;
let rows: RecordingRow[] = [];
let expanded: number | null = null;

/** How long after the last successful poll the recording kept running. */
function pollingStoppedEarlyBy(row: RecordingRow, d: RecordingDiagnostics): number | null {
  if (row.duration_s === null || d.last_game_time_s === null || d.alignment_offset_s === null) {
    return null;
  }
  return row.duration_s - (d.last_game_time_s + d.alignment_offset_s);
}

/**
 * What is worth saying about this recording, worst first. Empty means
 * nothing looked wrong.
 */
function concerns(row: RecordingRow, d: RecordingDiagnostics): string[] {
  const out: string[] = [];

  if (d.polls === 0) {
    out.push("The Live Client Data poller never got a single snapshot, so there are no markers, no samples and no champion.");
  } else if (!d.ever_matched) {
    out.push("We were never found in allPlayers — which is exactly why this recording has no champion, no KDA and an empty advantage curve.");
  }

  if (d.game_id === null) {
    out.push("The client never said which game this was, so queue, role and patch can never be filled in for it.");
  }

  if (d.alignment_offset_s === null && d.polls > 0) {
    out.push("The game clock was never seen to advance, so markers fall back to a 1:1 alignment and may sit in the wrong place.");
  }

  // The #74 fingerprint: polling died well before the recorder did.
  const gap = pollingStoppedEarlyBy(row, d);
  if (gap !== null && gap > 10) {
    out.push(`Polling stopped about ${gap.toFixed(0)}s before the recording did — the endpoint went away while the capture kept running.`);
  }

  if (/fail/i.test(d.backend)) {
    out.push(`The capture backend reported itself as "${d.backend}".`);
  }

  return out;
}

function parse(row: RecordingRow): RecordingDiagnostics | null {
  if (!row.diagnostics_json) return null;
  try {
    return JSON.parse(row.diagnostics_json) as RecordingDiagnostics;
  } catch {
    return null;
  }
}

function name(row: RecordingRow): string {
  return row.champion ?? row.path.split(/[\\/]/).pop() ?? `recording ${row.id}`;
}

function body(row: RecordingRow): string {
  const d = parse(row);
  if (!d) {
    return `<p class="hint-block">No record. Either this predates migration 7, a rescan imported
      it from a file we did not record, or serializing it failed at finalize.</p>`;
  }

  const problems = concerns(row, d);
  const banner = problems.length
    ? `<div class="warnbar warnbar-danger"><ul style="margin:0;padding-left:1.1rem">${problems
        .map((p) => `<li>${escapeHtml(p)}</li>`)
        .join("")}</ul></div>`
    : `<div class="warnbar">Nothing looked wrong with this one.</div>`;

  const facts: Array<[string, string]> = [
    ["Game id", d.game_id === null ? "—" : String(d.game_id)],
    ["Queue id", d.queue_id === null ? "—" : String(d.queue_id)],
    ["Custom", d.is_custom ? "yes" : "no"],
    ["Polls", String(d.polls)],
    [
      "Game clock seen",
      d.first_game_time_s === null || d.last_game_time_s === null
        ? "—"
        : `${duration(d.first_game_time_s)} → ${duration(d.last_game_time_s)}`,
    ],
    ["Matched in allPlayers", d.ever_matched ? "yes" : "no"],
    [
      "Alignment offset",
      d.alignment_offset_s === null ? "never proven" : `${d.alignment_offset_s.toFixed(2)}s`,
    ],
    ["Capture backend", d.backend],
    ["Markers written", String(d.markers)],
    ["Samples written", String(d.samples)],
    ["Video duration", duration(row.duration_s)],
  ];

  return `${banner}
    ${kv(facts)}
    ${expanded === row.id ? output(d) : ""}
    <button type="button" class="ghost" data-raw="${row.id}">${
      expanded === row.id ? "Hide" : "Show"
    } raw record</button>`;
}

function draw() {
  const host = root?.querySelector<HTMLElement>("#diag-list");
  if (!host) return;
  if (rows.length === 0) {
    host.innerHTML = `<div class="dev-empty">No recordings yet. Seed some, or finalize one.</div>`;
    return;
  }
  host.innerHTML = rows
    .map((row) =>
      card(
        `${escapeHtml(name(row))} <span class="hint">· ${escapeHtml(timestamp(row.started_at))}</span>`,
        body(row),
        "",
        true,
      ),
    )
    .join("");
}

async function load() {
  const result = await tryCall<RecordingRow[]>("list_recordings");
  if (!result.ok) {
    toast(result.error, "err");
    return;
  }
  // Newest first, and capped: this is a diagnostic panel, not the library.
  rows = result.value.slice(0, 25);
  draw();
}

export const diagnosticsPanel: Panel = {
  id: "diagnostics",
  title: "Diagnostics",
  icon: "◍",
  group: "Tools",

  mount(el) {
    root = el;
    el.innerHTML =
      panelHead(
        "Diagnostics",
        "What each finalize observed, as against what the recording contains — the 25 most recent.",
      ) +
      `<div class="row" style="margin-bottom:.8rem">
        <button type="button" class="ghost" data-reload>Reload</button>
      </div>
      <div id="diag-list"></div>`;

    el.addEventListener("click", (e) => {
      const target = e.target as HTMLElement;
      if (target.closest("[data-reload]")) {
        void load();
        return;
      }
      const raw = target.closest<HTMLElement>("[data-raw]")?.dataset.raw;
      if (raw) {
        const id = Number(raw);
        expanded = expanded === id ? null : id;
        draw();
      }
    });

    void load();
  },

  unmount() {
    root = null;
  },
};
