<!--
  What each recording's finalize actually observed, as against what the
  recording contains.

  The library shows you a card. This shows you why the card says what it says,
  or why it says nothing. The reasoning is in `lib/dev/diagnostics.ts`, where it
  has tests.
-->

<script lang="ts">
import { tryCall } from "../../../../dev/ipc";
import type { RecordingRow } from "../../../../dev/types";
import { concerns, parseDiagnostics, recordingName } from "../../../dev/diagnostics";
import { duration, timestamp } from "../../../dev/format";
import { devToast } from "../../../stores/devToast.svelte";
import Card from "../Card.svelte";
import KeyValues from "../KeyValues.svelte";
import Output from "../Output.svelte";
import PanelHead from "../PanelHead.svelte";

let rows = $state<RecordingRow[]>([]);
let expanded = $state<number | null>(null);

async function load() {
  const result = await tryCall<RecordingRow[]>("list_recordings");
  if (!result.ok) {
    devToast(result.error, "err");
    return;
  }
  // Newest first, and capped: this is a diagnostic panel, not the library.
  rows = result.value.slice(0, 25);
}

$effect(() => {
  void load();
});

function facts(row: RecordingRow): Array<[string, string]> {
  const d = parseDiagnostics(row);
  if (!d) return [];
  return [
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
}
</script>

<PanelHead
  title="Diagnostics"
  description="What each finalize observed, as against what the recording contains &mdash; the 25 most recent."
/>

<div class="row" style="margin-bottom:.8rem">
  <button type="button" class="ghost" onclick={() => void load()}>Reload</button>
</div>

{#if rows.length === 0}
  <div class="dev-empty">No recordings yet. Seed some, or finalize one.</div>
{:else}
  {#each rows as row (row.id)}
    {@const d = parseDiagnostics(row)}
    <Card title="{recordingName(row)} · {timestamp(row.started_at)}" raw>
      {#if !d}
        <p class="hint-block">
          No record. Either this predates migration 7, a rescan imported it from a file we did not
          record, or serializing it failed at finalize.
        </p>
      {:else}
        {@const problems = concerns(row, d)}
        {#if problems.length}
          <div class="warnbar warnbar-danger">
            <ul style="margin:0;padding-left:1.1rem">
              {#each problems as problem (problem)}
                <li>{problem}</li>
              {/each}
            </ul>
          </div>
        {:else}
          <div class="warnbar">Nothing looked wrong with this one.</div>
        {/if}

        <KeyValues pairs={facts(row)} />

        {#if expanded === row.id}
          <Output value={d} />
        {/if}
        <button
          type="button"
          class="ghost"
          onclick={() => (expanded = expanded === row.id ? null : row.id)}
        >
          {expanded === row.id ? "Hide" : "Show"} raw record
        </button>
      {/if}
    </Card>
  {/each}
{/if}
