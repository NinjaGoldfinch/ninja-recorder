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
import type {
  BackfillReport,
  FieldComparison,
  LcuComparison,
  Provenance,
  RecordingReport,
  RecordingRow,
} from "../types";
import { bytes, card, duration, escapeHtml, kv, output, panelHead, timestamp } from "../ui";

let root: HTMLElement | null = null;
let rows: RecordingRow[] = [];
let selected: number | null = null;
let report: RecordingReport | null = null;
let reportError: string | null = null;

// The client's answer is fetched on demand, never with the report: it needs a
// running League client, and a panel that failed to open without one would be
// useless for the offline half of what it shows.
let comparison: LcuComparison | null = null;
let comparisonError: string | null = null;
let asking = false;

// The last action's result, shown until another recording is opened. Actions
// are rare and deliberate, so the answer stays put rather than flashing a
// toast that is gone before it has been read.
let actionResult: string | null = null;
let actionError: string | null = null;
let running: string | null = null;

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

/** What each verdict means, said once. */
const VERDICT_NOTE: Record<string, string> = {
  agree: "Both answered, and they match.",
  differ: "Both answered, and they do not.",
  only_stored: "Only the row has it — the client did not answer for this field.",
  only_live: "Only the client has it — the row's column is empty.",
  neither: "Neither knows.",
};

/**
 * The row beside what the client says about the same game.
 *
 * Leads with the count of disagreements rather than making somebody scan for
 * them, because a disagreement almost always means the wrong `game_id` was
 * matched — which is exactly the thing that silently mislabels a library.
 */
function lcu(): string {
  const button = `<button type="button" class="ghost" data-ask ${asking ? "disabled" : ""}>${
    asking ? "Asking…" : comparison || comparisonError ? "Ask again" : "Ask the client"
  }</button>`;

  if (comparisonError !== null) {
    return card("What the client says now", button + output(comparisonError, true));
  }
  if (comparison === null) {
    return card(
      "What the client says now",
      button +
        `<p class="hint">Fetches the same one-shot <code>fetch_match_summary</code> the deferred
         patch uses and lays it beside the row. Needs the League client running, and the game
         still in its match history.</p>`,
    );
  }

  const c = comparison;
  const rowsHtml = c.fields
    .map((f: FieldComparison) => {
      const dash = "<span class='hint'>—</span>";
      const mark =
        f.verdict === "differ" ? "&#9888;" : f.verdict === "agree" ? "&#10003;" : "";
      return `<tr class="verdict-${escapeHtml(f.verdict)}">
        <td><code>${escapeHtml(f.field)}</code></td>
        <td>${f.stored === null ? dash : escapeHtml(f.stored)}</td>
        <td>${f.live === null ? dash : escapeHtml(f.live)}</td>
        <td class="hint">${mark} ${escapeHtml(VERDICT_NOTE[f.verdict] ?? f.verdict)}</td>
      </tr>`;
    })
    .join("");

  const verdict =
    c.differing === 0
      ? `<p class="hint">Nothing disagrees. Every field the client answered for matches the row.</p>`
      : `<p><strong>${c.differing} field${c.differing === 1 ? "" : "s"} disagree${
          c.differing === 1 ? "s" : ""
        }.</strong> Two views of one match should never differ, so this most likely means the
        wrong game was matched to this recording — worth checking before it mislabels the
        library.</p>`;

  return card(
    "What the client says now",
    button +
      verdict +
      `<table class="kv-table">
        <thead><tr><th>Field</th><th>Row</th><th>Client</th><th></th></tr></thead>
        <tbody>${rowsHtml}</tbody>
      </table>
      <p class="hint">Game <code>${c.game_id}</code>. Read-only — this writes nothing back.
      <code>dev_patch_match_summary</code> is the one that acts on the answer.</p>`,
  );
}

/**
 * The things you can do to this recording.
 *
 * **The report above stays a pure read; this block is the part that acts.**
 * That split is the point rather than a layout choice — opening the inspector
 * still cannot change what it describes, and only a press does.
 *
 * Every button is an existing command pointed at the row in front of you.
 * None of them is a new answer to a question something else already answers,
 * which is why #99 could describe this as wiring rather than a feature.
 */
function actions(r: RecordingReport): string {
  const busy = running !== null;
  const b = (action: string, label: string, extra = "") =>
    `<button type="button" data-run="${action}" ${busy ? "disabled" : ""} ${extra}>${
      running === action ? "Working…" : escapeHtml(label)
    }</button>`;

  // The patch needs a game to ask about. Without one there is nothing to
  // re-run, and a disabled button that says why beats one that fails.
  const hasGame = r.row.game_id !== null;

  const result =
    actionError !== null
      ? output(actionError, true)
      : actionResult !== null
        ? output(actionResult)
        : "";

  return card(
    "Act on it",
    `<div class="row wrap">
       ${b("patch", "Re-run the deferred patch", hasGame ? "" : "disabled")}
       ${b("backfill", "Backfill this row")}
       ${b("trim", "Trim the loading screen")}
       ${b("play", "Open the file")}
       ${b("folder", "Show in folder")}
     </div>
     ${
       hasGame
         ? ""
         : `<p class="hint">The deferred patch is unavailable: this row has no
            <code>game_id</code>, so there is no game to ask about. The backfill is the
            one that works without one — it matches on the clock.</p>`
     }
     <p class="hint">These write. The report above does not, and re-reads itself once an
     action finishes so what you are looking at is what the row now says.</p>
     ${result}`,
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
    lcu() +
    actions(r) +
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

/**
 * Which request the panel is currently waiting on.
 *
 * Two clicks in the list whose responses land out of order would otherwise
 * leave the newer selection displaying the older recording's row, provenance
 * and counts — which in a panel whose whole purpose is doubting a value is the
 * worst failure available to it.
 */
let openToken = 0;

async function open(id: number) {
  const token = ++openToken;
  selected = id;
  report = null;
  reportError = null;
  // The client's answer is about one game. Left in place it would sit under
  // the next recording opened, which in a panel built for doubting values is
  // the worst thing it could do.
  comparison = null;
  comparisonError = null;
  asking = false;
  actionResult = null;
  actionError = null;
  running = null;
  paint();
  const result = await tryCall<RecordingReport>("dev_recording_report", {
    recordingId: id,
  });
  // A newer click has already taken over; this answer is about a recording
  // nobody is looking at any more.
  if (token !== openToken) return;
  if (result.ok) {
    report = result.value;
  } else {
    // Shown rather than swallowed: "no recording 12" is the answer when a row
    // was deleted between listing it and opening it, and that is worth seeing.
    reportError = result.error;
  }
  paint();
}

/**
 * Asks the client about the selected recording.
 *
 * Guarded by the same token as `open`, for the same reason: this one is a
 * network round trip to a local process that may be busy, so an answer can
 * easily land after somebody has moved on to another row.
 */
async function ask() {
  const id = selected;
  if (id === null || asking) return;
  const token = openToken;

  asking = true;
  comparisonError = null;
  paint();

  const result = await tryCall<LcuComparison>("dev_recording_vs_lcu", { recordingId: id });
  if (token !== openToken) return;

  asking = false;
  if (result.ok) {
    comparison = result.value;
  } else {
    // Shown rather than swallowed: "League Client not running" and "this
    // recording has no game id" are both answers, and both are the reason
    // somebody opened this panel.
    comparison = null;
    comparisonError = result.error;
  }
  paint();
}

/**
 * Runs one action against the selected recording, then re-reads the report.
 *
 * The re-read is the important half. Every one of these writes, and an
 * inspector still showing the values from before the write would be worse
 * than one that showed nothing — the whole panel exists to be trusted about
 * what a row currently says.
 */
async function run(action: string) {
  const id = selected;
  if (id === null || running !== null || report === null) return;
  const token = openToken;

  running = action;
  actionResult = null;
  actionError = null;
  paint();

  // A custom game never reaches match history, so asking for it costs a
  // request and a full retry cycle for a 404 that can never become a 200.
  // Queue 0 is Riot's own id for a custom; an unknown queue is treated as
  // not-custom, which is the same guess the finalize makes.
  const isCustom = report.row.queue === 0;

  const call = (): Promise<{ ok: boolean; value?: unknown; error?: string }> => {
    switch (action) {
      case "patch":
        return tryCall<boolean>("dev_patch_match_summary", {
          recordingId: id,
          gameId: report!.row.game_id,
          isCustom,
        });
      case "backfill":
        return tryCall<BackfillReport>("dev_backfill_recording", { recordingId: id });
      case "trim":
        return tryCall<unknown>("dev_trim_lead_in", { recordingId: id });
      case "play":
        return tryCall<null>("dev_reveal_recording", { recordingId: id, which: "play" });
      default:
        return tryCall<null>("dev_reveal_recording", { recordingId: id, which: "folder" });
    }
  };

  const result = await call();
  if (token !== openToken) return;
  running = null;

  if (!result.ok) {
    actionError = result.error ?? "failed";
    paint();
    return;
  }

  // `dev_patch_match_summary` answers with a bare boolean covering three
  // different outcomes, so it is worth saying which in words rather than
  // printing `false` and leaving the reader to guess.
  actionResult =
    action === "patch"
      ? result.value === true
        ? "Patched. The row below has been re-read."
        : "Nothing was written. The client never produced stats for this game, the row is gone, or the retry schedule gave up — the Log panel distinguishes those."
      : action === "play" || action === "folder"
        ? "Handed to the OS."
        : JSON.stringify(result.value, null, 2);

  // The row may have changed under us; the comparison was about the old one.
  comparison = null;
  comparisonError = null;
  const fresh = await tryCall<RecordingReport>("dev_recording_report", { recordingId: id });
  if (token !== openToken) return;
  if (fresh.ok) report = fresh.value;
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
      if (target.closest("[data-ask]")) {
        void ask();
        return;
      }
      const runBtn = target.closest<HTMLElement>("[data-run]");
      if (runBtn) {
        void run(String(runBtn.dataset.run));
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
