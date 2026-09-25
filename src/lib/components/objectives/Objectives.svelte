<!--
  The objectives list (WS9 P0): what is active, paused and retired, with
  create and retire. Enough to see what a promoted takeaway became and to
  manage it; P2 adds the evidence and the desktop widget.

  It also carries the spreadsheet import, because the objectives are the part
  of the spreadsheet a user sees first: the file is read here, parsed in
  `reviewform/sheet.ts`, and sent to the daemon as rows.
-->

<script lang="ts">
import { showView } from "../../../router";
import type { ObjectiveCategory, ObjectiveStatus } from "../../contract/types";
import { whenDaemonReachable } from "../../stores/daemon.svelte";
import {
  createObjective,
  type ImportOutcome,
  importSheet,
  loadObjectives,
  objectives,
  setObjectiveStatus,
} from "../../stores/objectives.svelte";

const TABS: { value: ObjectiveStatus; label: string }[] = [
  { value: "active", label: "Active" },
  { value: "paused", label: "Paused" },
  { value: "retired", label: "Retired" },
];

const CATEGORIES: ObjectiveCategory[] = ["macro", "lane", "mental", "mechanics", "other"];

let body = $state("");
let category = $state<ObjectiveCategory>("other");
let fileInput: HTMLInputElement | undefined = $state();
let importing = $state(false);
let outcome = $state<ImportOutcome | null>(null);

async function onFile(input: HTMLInputElement) {
  const file = input.files?.[0];
  input.value = "";
  if (!file) return;
  importing = true;
  try {
    outcome = await importSheet(await file.text());
  } finally {
    importing = false;
  }
}

$effect(() => {
  whenDaemonReachable(() => void loadObjectives());
});

async function submit(event: SubmitEvent) {
  event.preventDefault();
  if (await createObjective(body, category)) body = "";
}
</script>

<div class="view-header">
  <button type="button" class="back-btn" onclick={() => showView("library")}>&larr; Back</button>
  <h2>Objectives</h2>
</div>

<section class="settings-group">
  <h3>New objective</h3>
  <form class="objective-add" onsubmit={submit}>
    <input type="text" aria-label="Objective" placeholder="e.g. Ward river at 2:45" bind:value={body} />
    <select aria-label="Category" bind:value={category}>
      {#each CATEGORIES as c (c)}
        <option value={c}>{c}</option>
      {/each}
    </select>
    <button type="submit" class="primary" disabled={body.trim() === ""}>Add</button>
  </form>
  <p class="setting-hint">Active objectives are what each new game is reviewed against.</p>
</section>

<section class="settings-group">
  <div class="segmented" role="radiogroup" aria-label="Status">
    {#each TABS as tab (tab.value)}
      <button
        type="button"
        role="radio"
        aria-checked={objectives.filter === tab.value}
        onclick={() => {
          objectives.filter = tab.value;
        }}>{tab.label} ({objectives.count(tab.value)})</button
      >
    {/each}
  </div>

  {#if objectives.visible.length === 0}
    <p class="hint">Nothing {objectives.filter}.</p>
  {:else}
    <ul class="objective-list">
      {#each objectives.visible as objective (objective.id)}
        <li>
          <span class="objective-body">{objective.body}</span>
          <span class="hint">{objective.category}</span>
          {#if objective.status === "active"}
            <button type="button" class="ghost" onclick={() => void setObjectiveStatus(objective.id, "paused")}
              >Pause</button
            >
          {:else}
            <button type="button" class="ghost" onclick={() => void setObjectiveStatus(objective.id, "active")}
              >Activate</button
            >
          {/if}
          {#if objective.status !== "retired"}
            <button type="button" class="ghost" onclick={() => void setObjectiveStatus(objective.id, "retired")}
              >Retire</button
            >
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
</section>

<section class="settings-group">
  <h3>Import from the spreadsheet</h3>
  <p class="setting-hint">
    A CSV export with the columns date, time, block, game no, playing, matchup, game, lane,
    mental, clear time, smites, deaths, learning objectives, key takeaways and block takeaways.
    Dates are day first. Importing the same file twice changes nothing.
  </p>
  <input
    bind:this={fileInput}
    type="file"
    accept=".csv,text/csv"
    hidden
    aria-label="Spreadsheet CSV"
    onchange={(e) => void onFile(e.currentTarget)}
  />
  <button type="button" class="ghost" disabled={importing} onclick={() => fileInput?.click()}
    >{importing ? "Importing…" : "Import spreadsheet…"}</button
  >
  {#if outcome}
    <div class="import-outcome" role="status">
      {#if outcome.report}
        <p>
          {outcome.report.rows} rows: {outcome.report.games_created} new games,
          {outcome.report.games_matched} matched to games already here,
          {outcome.report.objectives_created} new objectives,
          {outcome.report.takeaways_created} new takeaways.
        </p>
      {/if}
      {#if outcome.errors.length > 0}
        <p class="import-errors-title">Not imported:</p>
        <ul class="import-errors">
          {#each outcome.errors as error (error.line)}
            <li>Line {error.line}: {error.message}</li>
          {/each}
        </ul>
      {/if}
    </div>
  {/if}
</section>
