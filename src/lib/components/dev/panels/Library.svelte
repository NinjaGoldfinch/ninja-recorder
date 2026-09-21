<!--
  One recording, from every source that knows something about it.

  The information already existed and was scattered — the Database panel has
  the raw row, Diagnostics has what the finalize observed, the Log has what the
  poller saw, the review view has the markers. This is the one place they sit
  together, which is what is actually wanted when a row looks wrong: a role
  that says Jungle for a top game, empty item slots, a missing champion (#99).

  **Provenance is the point.** `champion` and `role` have two possible writers
  each and a rule about which wins; `queue` comes from the gameflow session.
  Listing values alone would leave the reader to remember all of that, so every
  ambiguous field is shown with the thing that likely wrote it — and
  `role`/`patch` are the certain ones, because only the deferred patch writes
  them, which makes their absence evidence rather than a shrug.
-->

<script lang="ts">
import { tryCall } from "../../../../dev/ipc";
import type {
  BackfillReport,
  LcuComparison,
  RecordingReport,
  RecordingRow,
} from "../../../../dev/types";
import { bytes, duration, MISSING, timestamp } from "../../../dev/format";
import type { PanelProps } from "../../../dev/panels";
import Card from "../Card.svelte";
import KeyValues from "../KeyValues.svelte";
import Output from "../Output.svelte";
import PanelHead from "../PanelHead.svelte";

const { payload }: PanelProps = $props();

let rows = $state<RecordingRow[]>([]);
let selected = $state<number | null>(null);
let report = $state<RecordingReport | null>(null);
let reportError = $state<string | null>(null);

// The client's answer is fetched on demand, never with the report: it needs a
// running League client, and a panel that failed to open without one would be
// useless for the offline half of what it shows.
let comparison = $state<LcuComparison | null>(null);
let comparisonError = $state<string | null>(null);
let asking = $state(false);

// The last action's result, shown until another recording is opened. Actions
// are rare and deliberate, so the answer stays put rather than flashing a
// toast that is gone before it has been read.
let actionResult = $state<string | null>(null);
let actionError = $state<string | null>(null);
let running = $state<string | null>(null);

/**
 * Which request the panel is currently waiting on.
 *
 * Two clicks in the list whose responses land out of order would otherwise
 * leave the newer selection displaying the older recording's row, provenance
 * and counts — which in a panel whose whole purpose is doubting a value is the
 * worst failure available to it.
 */
let openToken = 0;

/** What each verdict means, said once. */
const VERDICT_NOTE: Record<string, string> = {
  agree: "Both answered, and they match.",
  differ: "Both answered, and they do not.",
  only_stored: "Only the row has it — the client did not answer for this field.",
  only_live: "Only the client has it — the row's column is empty.",
  neither: "Neither knows.",
};

$effect(() => {
  // Deep link from the main window's rows: `#/library/<id>` arrives as the
  // route payload, so the panel opens on the recording somebody was already
  // looking at rather than on an empty list.
  const wanted = Number(payload);
  void load().then(() => {
    if (Number.isFinite(wanted) && wanted > 0) void open(wanted);
  });
});

function title(row: RecordingRow): string {
  return row.champion ?? row.path.split(/[\\/]/).pop() ?? `recording ${row.id}`;
}

async function load() {
  const result = await tryCall<RecordingRow[]>("list_recordings");
  rows = result.ok ? result.value : [];
}

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

  const result = await tryCall<RecordingReport>("dev_recording_report", { recordingId: id });
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
}

/**
 * Runs one action against the selected recording, then re-reads the report.
 *
 * The re-read is the important half. Every one of these writes, and an
 * inspector still showing the values from before the write would be worse than
 * one that showed nothing — the whole panel exists to be trusted about what a
 * row currently says.
 */
async function run(action: string) {
  const id = selected;
  const current = report;
  if (id === null || running !== null || current === null) return;
  const token = openToken;

  running = action;
  actionResult = null;
  actionError = null;

  // A custom game never reaches match history, so asking for it costs a
  // request and a full retry cycle for a 404 that can never become a 200.
  // Queue 0 is Riot's own id for a custom; an unknown queue is treated as
  // not-custom, which is the same guess the finalize makes.
  const isCustom = current.row.queue === 0;

  const result = await (() => {
    switch (action) {
      case "patch":
        return tryCall<boolean>("dev_patch_match_summary", {
          recordingId: id,
          gameId: current.row.game_id,
          isCustom,
          // Gates the rank read. The row's own queue, so a re-run asks about
          // the ladder the game was actually played on — and a non-ranked
          // queue correctly reads no rank at all.
          queueId: current.row.queue,
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
  })();

  if (token !== openToken) return;
  running = null;

  if (!result.ok) {
    actionError = result.error;
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
}

/** The deferred patch needs a game to ask about. Without one there is nothing
 *  to re-run, and a disabled button that says why beats one that fails. */
const hasGame = $derived(report?.row.game_id !== null && report?.row.game_id !== undefined);

const ACTIONS: Array<[id: string, label: string]> = [
  ["patch", "Re-run the deferred patch"],
  ["backfill", "Backfill this row"],
  ["trim", "Trim the loading screen"],
  ["play", "Open the file"],
  ["folder", "Show in folder"],
];
</script>

<PanelHead
  title="Library"
  description="One recording from every source that knows something about it — the row, where each value came from, and what it carries."
/>

<div class="row" style="margin-bottom:.8rem">
  <button type="button" class="ghost" onclick={() => void load()}>Reload</button>
</div>

<div class="split">
  <Card title="Library">
    {#if rows.length === 0}
      <p class="hint">No recordings. The Seed panel writes some.</p>
    {:else}
      <div class="list">
        {#each rows as row (row.id)}
          <button
            type="button"
            class="list-row"
            class:selected={row.id === selected}
            onclick={() => void open(row.id)}
          >
            <span class="list-main">{title(row)}</span>
            <span class="hint">
              {timestamp(row.started_at)} · {duration(row.duration_s)} · {bytes(row.size_bytes)}
            </span>
          </button>
        {/each}
      </div>
    {/if}
  </Card>

  <div>
    {#if selected === null}
      <Card title="Recording"><p class="hint">Pick one on the left.</p></Card>
    {:else if reportError !== null}
      <Card title="Recording"><Output value={reportError} isError /></Card>
    {:else if report === null}
      <Card title="Recording"><p class="hint">Loading…</p></Card>
    {:else}
      <Card title="Row"><Output value={report.row} /></Card>

      <Card title="Where each value came from">
        <table class="kv-table">
          <thead>
            <tr><th>Field</th><th>Likely writer</th><th>Why</th></tr>
          </thead>
          <tbody>
            {#each report.provenance as p (p.field)}
              <tr>
                <td><code>{p.field}</code></td>
                <td>
                  {#if p.source}{p.source}{:else}<span class="hint">{MISSING}</span>{/if}
                </td>
                <td class="hint">{p.note}</td>
              </tr>
            {/each}
          </tbody>
        </table>
        <p class="hint">
          Derived from what the row looks like — nothing records a per-column writer, so this is a
          strong hint rather than an audit. <code>role</code> and <code>patch</code> are the certain
          ones: only the deferred patch writes them.
        </p>
      </Card>

      <!--
        The row beside what the client says about the same game.

        Leads with the count of disagreements rather than making somebody scan
        for them, because a disagreement almost always means the wrong
        `game_id` was matched — which is exactly the thing that silently
        mislabels a library.
      -->
      <Card title="What the client says now">
        <button type="button" class="ghost" disabled={asking} onclick={() => void ask()}>
          {asking ? "Asking…" : comparison || comparisonError ? "Ask again" : "Ask the client"}
        </button>

        {#if comparisonError !== null}
          <Output value={comparisonError} isError />
        {:else if comparison === null}
          <p class="hint">
            Fetches the same one-shot <code>fetch_match_summary</code> the deferred patch uses and
            lays it beside the row. Needs the League client running, and the game still in its match
            history.
          </p>
        {:else}
          {#if comparison.differing === 0}
            <p class="hint">
              Nothing disagrees. Every field the client answered for matches the row.
            </p>
          {:else}
            <p>
              <strong>
                {comparison.differing} field{comparison.differing === 1 ? "" : "s"} disagree{comparison.differing ===
                1
                  ? "s"
                  : ""}.
              </strong>
              Two views of one match should never differ, so this most likely means the wrong game was
              matched to this recording — worth checking before it mislabels the library.
            </p>
          {/if}

          <table class="kv-table">
            <thead>
              <tr><th>Field</th><th>Row</th><th>Client</th><th></th></tr>
            </thead>
            <tbody>
              {#each comparison.fields as f (f.field)}
                <tr class="verdict-{f.verdict}">
                  <td><code>{f.field}</code></td>
                  <td>
                    {#if f.stored === null}<span class="hint">{MISSING}</span>{:else}{f.stored}{/if}
                  </td>
                  <td>
                    {#if f.live === null}<span class="hint">{MISSING}</span>{:else}{f.live}{/if}
                  </td>
                  <td class="hint">
                    {f.verdict === "differ" ? "⚠" : f.verdict === "agree" ? "✓" : ""}
                    {VERDICT_NOTE[f.verdict] ?? f.verdict}
                  </td>
                </tr>
              {/each}
            </tbody>
          </table>

          <p class="hint">
            Game <code>{comparison.game_id}</code>. Read-only — this writes nothing back.
            <code>dev_patch_match_summary</code> is the one that acts on the answer.
          </p>
        {/if}
      </Card>

      <!--
        **The report above stays a pure read; this block is the part that
        acts.** That split is the point rather than a layout choice — opening
        the inspector still cannot change what it describes, and only a press
        does.

        Every button is an existing command pointed at the row in front of you.
        None of them is a new answer to a question something else already
        answers, which is why #99 could describe this as wiring rather than a
        feature.
      -->
      <Card title="Act on it">
        <div class="row wrap">
          {#each ACTIONS as [id, label] (id)}
            <button
              type="button"
              disabled={running !== null || (id === "patch" && !hasGame)}
              onclick={() => void run(id)}
            >
              {running === id ? "Working…" : label}
            </button>
          {/each}
        </div>

        {#if !hasGame}
          <p class="hint">
            The deferred patch is unavailable: this row has no <code>game_id</code>, so there is no
            game to ask about. The backfill is the one that works without one — it matches on the
            clock.
          </p>
        {/if}

        <p class="hint">
          These write. The report above does not, and re-reads itself once an action finishes so
          what you are looking at is what the row now says.
        </p>

        {#if actionError !== null}
          <Output value={actionError} isError />
        {:else if actionResult !== null}
          <Output value={actionResult} />
        {/if}
      </Card>

      <!--
        Gold is called out separately because it comes from a different source
        on a different schedule. Plenty of samples and no gold is the #137
        fingerprint — the live poller ran, the deferred patch never finished —
        rather than a contradiction.
      -->
      <Card title="What it carries">
        <KeyValues
          pairs={[
            ["Markers", String(report.markers)],
            ["Samples", String(report.samples)],
            ["…with gold", String(report.gold_samples)],
            [
              "Alignment offset",
              report.alignment_offset_s === null
                ? null
                : `${report.alignment_offset_s.toFixed(2)}s`,
            ],
          ]}
        />
        {#if report.samples > 0 && report.gold_samples === 0}
          <p class="hint">
            Samples but no gold: the live poller ran and the deferred patch never landed. Settings →
            Storage → fill in can recover it while the client still remembers the game.
          </p>
        {/if}
      </Card>

      <Card title="Scoreboard">
        {#if report.scoreboard === null}
          <p class="hint">
            None. A game whose poller never saw a player list, or a file
            <code>reconcile</code> imported.
          </p>
        {:else}
          <Output value={report.scoreboard} />
        {/if}
      </Card>

      <Card title="Diagnostics">
        {#if report.diagnostics === null}
          <p class="hint">
            None. Anything recorded before migration 7, and anything <code>reconcile</code>
            imported.
          </p>
        {:else}
          <Output value={report.diagnostics} />
        {/if}
      </Card>
    {/if}
  </div>
</div>
