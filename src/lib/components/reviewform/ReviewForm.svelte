<!--
  The review form for one game (WS9 P0), in the rail beside the player since
  P1 (`review/ReviewRail.svelte`).

  Every field saves itself: the review is edited as a draft and written whole
  after a short pause (`reviewform/autosave.ts`), and ticks and takeaways are
  saved as they happen. The rail's tab bar says which state the draft is in.

  **What the recording already knows is filled in** for a review never saved
  (`reviewform/autofill.ts`), and tagged "auto" until the user changes it.

  **No `{@html}`.** Objective and takeaway text is the user's, and the hints
  come from the recording; default interpolation escapes all of it.
-->

<script lang="ts">
import { formatClock, parseClock, parseCount } from "../../reviewform/fields";
import { GAME_CHOICES, LANE_CHOICES, MENTAL_CHOICES } from "../../reviewform/ratings";
import {
  addTakeaway,
  deleteTakeaway,
  edit,
  gameReview,
  promoteTakeaway,
  setTicked,
} from "../../stores/gameReview.svelte";
import RatingControl from "./RatingControl.svelte";

interface Props {
  /**
   * The game clock at the player's position, in seconds, or null when the
   * recording has nothing to measure it from. Asked for on demand rather than
   * passed as a number: the playhead moves sixty times a second, and the form
   * only wants it when the clock button is pressed.
   */
  gameClockNow?: () => number | null;
}

const { gameClockNow = () => null }: Props = $props();

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

/** Pause on the clear, press the clock: the field takes the game time. */
function clearFromPlayhead() {
  const seconds = gameClockNow();
  if (seconds === null) return;
  onClear(formatClock(Math.floor(seconds) * 1000));
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

/** Enter adds the takeaway, as in a chat box; Shift+Enter is a new line. */
function takeawayKey(event: KeyboardEvent) {
  if (event.key !== "Enter" || event.shiftKey || event.isComposing) return;
  event.preventDefault();
  (event.currentTarget as HTMLTextAreaElement).form?.requestSubmit();
}

// Only while the box is blank: that is when the markers are what counts.
const deathsHint = $derived.by(() => {
  if (deathsText !== "") return null;
  const markers = gameReview.current?.death_markers ?? null;
  if (markers === null) return "No death markers to count for this game.";
  return `Blank uses the ${markers} counted from the recording.`;
});
</script>

{#if gameReview.current}
  <div class="review-form">
    <section class="review-section">
      <RatingControl
        label="Game"
        choices={GAME_CHOICES}
        value={gameReview.draft.game_rating}
        auto={gameReview.isAuto("game_rating")}
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
        <div class="field" class:invalid={clearInvalid}>
          <label class="field-label" for="review-clear">Clear time</label>
          <span class="field-with-button">
            <input
              id="review-clear"
              type="text"
              inputmode="numeric"
              placeholder="m:ss"
              aria-invalid={clearInvalid}
              value={clearText}
              oninput={(e) => onClear(e.currentTarget.value)}
            />
            <button
              type="button"
              class="icon-btn clock-btn"
              aria-label="Use the game time at the playhead"
              title="Use the game time at the playhead"
              onclick={clearFromPlayhead}>⏱</button
            >
          </span>
        </div>
        <div class="field" class:invalid={smitesInvalid}>
          <label class="field-label" for="review-smites">Smites</label>
          <input
            id="review-smites"
            type="text"
            inputmode="numeric"
            aria-invalid={smitesInvalid}
            value={smitesText}
            oninput={(e) => onSmites(e.currentTarget.value)}
          />
        </div>
        <div class="field" class:invalid={deathsInvalid}>
          <label class="field-label" for="review-deaths"
            >Deaths
            {#if gameReview.isAuto("deaths")}
              <span class="auto-tag" title="From the end-of-game stats">auto</span>
            {/if}</label
          >
          <input
            id="review-deaths"
            type="text"
            inputmode="numeric"
            placeholder={gameReview.current.death_markers === null
              ? ""
              : `${gameReview.current.death_markers} (auto)`}
            aria-invalid={deathsInvalid}
            value={deathsText}
            oninput={(e) => onDeaths(e.currentTarget.value)}
          />
        </div>
      </div>
      {#if deathsHint}<p class="field-hint">{deathsHint}</p>{/if}
    </section>

    <section class="review-section">
      <h3>Reviewing against</h3>
      {#if gameReview.current.objectives.length === 0}
        <p class="field-hint">No objectives were active when this game started.</p>
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

    <section class="review-section">
      <h3>Takeaways</h3>
      {#if gameReview.current.takeaways.length > 0}
        <ul class="review-takeaways">
          {#each gameReview.current.takeaways as takeaway (takeaway.id)}
            <li>
              <span class="takeaway-body">{takeaway.body}</span>
              {#if takeaway.promoted_to_id === null}
                <button
                  type="button"
                  class="icon-btn"
                  aria-label="Promote to objective"
                  title="Promote to objective"
                  onclick={() => void promoteTakeaway(takeaway.id)}>⤴</button
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
        <textarea
          rows="1"
          aria-label="New takeaway"
          placeholder="What will you do differently?"
          title="Enter adds it; Shift+Enter starts a new line"
          bind:value={newTakeaway}
          onkeydown={takeawayKey}
        ></textarea>
        <button type="submit" class="primary" disabled={newTakeaway.trim() === ""}>Add</button>
      </form>
    </section>

    <section class="review-section review-notes-section">
      <h3>Notes</h3>
      <textarea
        class="review-notes"
        aria-label="Notes"
        placeholder="Anything worth remembering about this game."
        value={gameReview.draft.free_notes}
        oninput={(e) => edit({ free_notes: e.currentTarget.value })}
      ></textarea>
    </section>
  </div>
{:else if gameReview.loading}
  <p class="hint">Loading…</p>
{:else}
  <p class="hint">No game is open.</p>
{/if}
