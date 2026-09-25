<!--
  The objectives list (WS9 P0): what is active, paused and retired, with
  create and retire. Enough to see what a promoted takeaway became and to
  manage it; P2 adds the evidence and the desktop widget.
-->

<script lang="ts">
import { showView } from "../../../router";
import type { ObjectiveCategory, ObjectiveStatus } from "../../contract/types";
import { whenDaemonReachable } from "../../stores/daemon.svelte";
import {
  createObjective,
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
