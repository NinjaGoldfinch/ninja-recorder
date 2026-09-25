<!--
  The review form for one game (WS9 P0): the right-hand rail of the review
  screen, without the video. P1 puts it beside the player.

  Every field saves itself: the review is edited as a draft and written whole
  after a short pause (`reviewform/autosave.ts`), and ticks and takeaways are
  saved as they happen. The indicator in the header says which state the draft
  is in.

  **No `{@html}`.** Objective and takeaway text is the user's, and the header's
  champion names come from a scoreboard; default interpolation escapes all of it.
-->

<script lang="ts">
import { formatDateTime } from "../../../format";
import { formatClock, parseClock, parseCount } from "../../reviewform/fields";
import { GAME_CHOICES, LANE_CHOICES, MENTAL_CHOICES } from "../../reviewform/ratings";
import {
  addTakeaway,
  closeReview,
  deleteTakeaway,
  edit,
  gameReview,
  promoteTakeaway,
  setTicked,
} from "../../stores/gameReview.svelte";
import RatingControl from "./RatingControl.svelte";

const STATUS_COPY = {
  saved: "Saved",
  unsaved: "Unsaved changes",
  saving: "Saving…",
  error: "Not saved",
} as const;

// Text boxes keep what was typed, even when it does not parse yet, so a
// half-typed "2:" is not wiped by the next render. They are reset from the
// saved review whenever a different game is opened.
let clearText = $state("");
let smitesText = $state("");
let deathsText = $state("");
let clearInvalid = $state(false);
let smitesInvalid = $state(false);
let deathsInvalid = $state(false);
let newTakeaway = $state("");
let shownGame: number | null = null;

$effect(() => {
  const game = gameReview.current;
  if (!game || game.game.id === shownGame) return;
  shownGame = game.game.id;
  const review = gameReview.draft;
  clearText = formatClock(review.first_clear_ms);
  smitesText = review.smites_at_clear === null ? "" : String(review.smites_at_clear);
  deathsText = review.deaths === null ? "" : String(review.deaths);
  clearInvalid = smitesInvalid = deathsInvalid = false;
  newTakeaway = "";
});

function onClear(text: string) {
  clearText = text;
  const parsed = parseClock(text);
  clearInvalid = !parsed.ok;
  if (parsed.ok) edit({ first_clear_ms: parsed.value });
}

function onSmites(text: string) {
  smitesText = text;
  const parsed = parseCount(text);
  smitesInvalid = !parsed.ok;
  if (parsed.ok) edit({ smites_at_clear: parsed.value });
}

function onDeaths(text: string) {
  deathsText = text;
  const parsed = parseCount(text);
  deathsInvalid = !parsed.ok;
  if (parsed.ok) edit({ deaths: parsed.value });
}

async function submitTakeaway(event: SubmitEvent) {
  event.preventDefault();
  if (await addTakeaway(newTakeaway)) newTakeaway = "";
}

const deathsHint = $derived.by(() => {
  const markers = gameReview.current?.death_markers ?? null;
  if (markers === null) return "No death markers to count for this game.";
  return `Leave blank to use the ${markers} counted from the recording.`;
});
</script>

<div class="view-header">
  <button type="button" class="back-btn" onclick={() => void closeReview()}>&larr; Back</button>
  <h2>Review</h2>
  <span class="save-status" data-status={gameReview.status} role="status" aria-live="polite"
    >{STATUS_COPY[gameReview.status]}</span
  >
</div>

{#if gameReview.current}
  {@const game = gameReview.current.game}
  <section class="settings-group review-form-head">
    <p class="review-game-title">
      <span>{formatDateTime(game.started_at)}</span>
      {#if game.result}
        <span class="result-pill" data-outcome={game.result}
          >{game.result === "win" ? "Win" : "Loss"}</span
        >
      {/if}
      <span>{game.champion ?? "Unknown champion"}</span>
      {#if game.matchup}<span class="hint">vs {game.matchup}</span>{/if}
    </p>
  </section>

  <section class="settings-group">
    <h3>Ratings</h3>
    <RatingControl
      label="Game"
      choices={GAME_CHOICES}
      value={gameReview.draft.game_rating}
      onchange={(v) => edit({ game_rating: v })}
    />
    <RatingControl
      label="Lane"
      choices={LANE_CHOICES}
      value={gameReview.draft.lane_rating}
      onchange={(v) => edit({ lane_rating: v })}
    />
    <RatingControl
      label="Mental"
      choices={MENTAL_CHOICES}
      value={gameReview.draft.mental_rating}
      onchange={(v) => edit({ mental_rating: v })}
    />

    <div class="review-numbers">
      <label class="field" class:invalid={clearInvalid}>
        <span class="setting-label">Clear time</span>
        <input
          type="text"
          inputmode="numeric"
          placeholder="m:ss"
          aria-invalid={clearInvalid}
          value={clearText}
          oninput={(e) => onClear(e.currentTarget.value)}
        />
      </label>
      <label class="field" class:invalid={smitesInvalid}>
        <span class="setting-label">Smites at clear</span>
        <input
          type="text"
          inputmode="numeric"
          aria-invalid={smitesInvalid}
          value={smitesText}
          oninput={(e) => onSmites(e.currentTarget.value)}
        />
      </label>
      <label class="field" class:invalid={deathsInvalid}>
        <span class="setting-label">Deaths</span>
        <input
          type="text"
          inputmode="numeric"
          placeholder={gameReview.current.death_markers === null
            ? ""
            : `${gameReview.current.death_markers} (auto)`}
          aria-invalid={deathsInvalid}
          value={deathsText}
          oninput={(e) => onDeaths(e.currentTarget.value)}
        />
        <span class="hint">{deathsHint}</span>
      </label>
    </div>
  </section>

  <section class="settings-group">
    <h3>Reviewing against</h3>
    {#if gameReview.current.objectives.length === 0}
      <p class="hint">No objectives were active when this game started.</p>
    {:else}
      <ul class="review-checklist">
        {#each gameReview.current.objectives as objective (objective.objective_id)}
          <li>
            <label class="check">
              <input
                type="checkbox"
                checked={objective.ticked}
                onchange={(e) => void setTicked(objective.objective_id, e.currentTarget.checked)}
              />
              <span>{objective.body}</span>
              {#if objective.status !== "active"}
                <span class="hint">({objective.status})</span>
              {/if}
            </label>
          </li>
        {/each}
      </ul>
    {/if}
  </section>

  <section class="settings-group">
    <h3>Takeaways</h3>
    {#if gameReview.current.takeaways.length > 0}
      <ul class="review-takeaways">
        {#each gameReview.current.takeaways as takeaway (takeaway.id)}
          <li>
            <span class="takeaway-body">{takeaway.body}</span>
            {#if takeaway.promoted_to_id === null}
              <button type="button" class="ghost" onclick={() => void promoteTakeaway(takeaway.id)}
                >Promote to objective</button
              >
            {:else}
              <span class="hint">Promoted</span>
            {/if}
            <button
              type="button"
              class="icon-btn danger"
              aria-label="Delete takeaway"
              title="Delete takeaway"
              onclick={() => void deleteTakeaway(takeaway.id)}>🗑</button
            >
          </li>
        {/each}
      </ul>
    {/if}
    <form class="takeaway-add" onsubmit={submitTakeaway}>
      <textarea rows="2" aria-label="New takeaway" placeholder="What will you do differently?"
        bind:value={newTakeaway}
      ></textarea>
      <button type="submit" class="primary" disabled={newTakeaway.trim() === ""}>Add</button>
    </form>
  </section>

  <section class="settings-group">
    <h3>Notes</h3>
    <textarea
      class="review-notes"
      rows="5"
      aria-label="Notes"
      value={gameReview.draft.free_notes}
      oninput={(e) => edit({ free_notes: e.currentTarget.value })}
    ></textarea>
  </section>
{:else if gameReview.loading}
  <p class="hint">Loading…</p>
{:else}
  <p class="hint">No game is open.</p>
{/if}
