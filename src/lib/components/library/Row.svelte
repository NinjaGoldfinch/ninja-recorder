<!--
  One recording in the library.

  **No `{@html}` anywhere in this file, and that is the point of the
  migration.** `db::reconcile` imports whatever video file the user drops into
  the recordings folder, so a filename is untrusted input and `vodTitle` falls
  back to it. v1 guarded every one of these with `escapeHtml` / `escapeAttr`
  around a template string; Svelte's default text interpolation replaces both,
  and `{@html}` opts straight back out of the thing that made this safe.

  **Every column can be absent, and a row that hid the slot when it is would
  read as a different shape per recording**, which is exactly what makes a list
  scannable or not. An em dash keeps the lines where they are and says so out
  loud. `frontend.md`'s fallback table is the specification for this.
-->

<script lang="ts">
import {
  formatBytes,
  formatClock,
  formatDateTime,
  formatKda,
  formatRelative,
  kdaRatio,
  lpLabel,
  patchLabel,
  queueOrModeLabel,
  rankLabel,
  vodHeading,
  vodTitle,
} from "../../../format";
import type { RecordingRow } from "../../../types";
import { recordedWithout } from "../../library/problems";
import { csPerMinute, laneOpponent, outcomeAttr } from "../../library/scoreboard";
import Loadout from "./Loadout.svelte";
import Matchup from "./Matchup.svelte";
import RowActions from "./RowActions.svelte";
import Slot from "./Slot.svelte";

interface Props {
  row: RecordingRow;
  onopen: (row: RecordingRow) => void;
  onreview: (row: RecordingRow) => void;
  onpin: (row: RecordingRow) => void;
  ondelete: (row: RecordingRow) => void;
  oninspect: (row: RecordingRow) => void;
  showInspect: boolean;
}

const { row, onopen, onreview, onpin, ondelete, oninspect, showInspect }: Props = $props();

const title = $derived(vodTitle(row));
const kda = $derived(formatKda(row.kda_k, row.kda_d, row.kda_a));
const ratio = $derived(kdaRatio(row.kda_k, row.kda_d, row.kda_a));
const patch = $derived(patchLabel(row.patch));
const rank = $derived(rankLabel(row.tier, row.division));
// Only beside a rank. LP with no tier is a number with no scale, and the
// column can be in that state: `fill_ranked` writes all three together, but
// a row from before migration 10 has neither.
const lp = $derived(rank === null ? null : lpLabel(row.lp_after));
const length = $derived(row.duration_s === null ? null : formatClock(row.duration_s));

// Said once, in the left block, and shown once as the leading accent. The
// word is what makes the row readable without colour, since green and red
// are exactly the pair a red-green deficiency cannot separate, and it costs
// no column because that block is already a run of lines.
//
// Absent when the result is unknown, which is unambiguous rather than a gap:
// a word is on every decided row, so no word means undecided.
const outcomeWord = $derived(row.win === null ? null : row.win ? "Win" : "Loss");

// What a capture failure cost this recording (#10), from its diagnostics:
// short in the row, every reason in the tooltip. In the slack column, which is
// otherwise empty, so a row that says it is still the same shape as one that
// does not.
const without = $derived(recordedWithout(row.diagnostics_json));

function onCardKey(e: KeyboardEvent) {
  if (e.key !== "Enter" && e.key !== " ") return;
  // The actions are real buttons and are reachable by Tab; `:focus-within`
  // is what makes them visible there. Enter on a focused button has to stay
  // the button's, so this must not swallow it.
  if ((e.target as HTMLElement).closest("button")) return;
  e.preventDefault();
  onopen(row);
}
</script>

<!--
  **Inherited, and flagged rather than quietly redesigned.** A focusable,
  clickable `role="listitem"` is what v1's markup already did, and the whole
  row being the click target is the interaction people are used to. ARIA has no
  good role for an activatable list item, so the honest options are to leave it
  as it is or to put a real button inside every row, which is a UX change and
  not what WS4.3 is for. Worth its own issue; not worth a silent decision here.

  The keyboard half is real and is not suppressed: `onCardKey` handles Enter
  and Space and steps aside for the buttons inside.
-->
<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<article
  class="vod-row"
  role="listitem"
  tabindex="0"
  data-id={row.id}
  data-outcome={outcomeAttr(row.win)}
  aria-label={vodHeading(row, laneOpponent(row)?.champion ?? null)}
  onclick={() => onopen(row)}
  onkeydown={onCardKey}
>
  <span class="vod-meta">
    <span class="vod-queue">
      {#if queueOrModeLabel(row) === null}
        <span class="vod-missing">&mdash;</span>
      {:else}
        <span>{queueOrModeLabel(row)}</span>
      {/if}
    </span>
    <time
      datetime={new Date(row.started_at).toISOString()}
      class="vod-sub"
      title={formatDateTime(row.started_at)}>{formatRelative(row.started_at)}</time
    >
    <span class="vod-sub">
      {#if patch === null}&nbsp;{:else}Patch {patch}{/if}
    </span>
    <span class="vod-sub">
      {#if length === null}
        <span class="vod-missing">&mdash;</span>
      {:else}
        <span>{length}</span>
      {/if}{#if outcomeWord}
        &middot; <span class="vod-outcome">{outcomeWord}</span>
      {/if}
    </span>
  </span>

  <span class="vod-portrait" aria-hidden="true">
    <Slot kind="champion" key={row.champion ?? ""} />
  </span>

  <span class="vod-cell">
    <span class="vod-champ" {title}>{title}</span>
    <span class="vod-sub">
      {#if row.role === null}
        <span class="vod-missing">Unknown</span>
      {:else}{row.role}{/if}
    </span>
  </span>

  <span class="vod-cell">
    <span class="vod-value vod-kda">
      <!--
        Deaths in their own colour, which is the one number on a row people
        look for first. Built from the three integers rather than by splitting
        `formatKda`'s string, so nothing here parses its own output. But
        `formatKda` still owns the all-three-or-nothing rule, which is why the
        condition is on it.
      -->
      {#if kda === null}
        <span class="vod-missing">&mdash;</span>
      {:else}
        {row.kda_k}
        <span class="vod-slash">/</span>
        <span class="vod-deaths">{row.kda_d}</span>
        <span class="vod-slash">/</span>
        {row.kda_a}
      {/if}
    </span>
    <span class="vod-sub">{#if ratio}{ratio}{:else}&nbsp;{/if}</span>
  </span>

  <span class="vod-cell">
    <span class="vod-value">
      {#if row.cs === null}
        <span class="vod-missing">&mdash;</span>
      {:else}
        <span>{row.cs} cs</span>
      {/if}
    </span>
    <span class="vod-sub">{#if csPerMinute(row)}{csPerMinute(row)}{:else}&nbsp;{/if}</span>
  </span>

  <!--
    The rank the game was played at. Dashed on everything that had no ladder,
    like every other column: most libraries hold a mix of ranked and unranked
    games.
  -->
  <span class="vod-cell">
    <span class="vod-value">
      {#if rank === null}
        <span class="vod-missing">&mdash;</span>
      {:else}
        <span title="The rank this game was played at">{rank}</span>
      {/if}
    </span>
    <span class="vod-sub">{#if lp === null}&nbsp;{:else}{lp}{/if}</span>
  </span>

  <Loadout {row} />

  <Matchup {row} />

  {#if without}
    <span class="vod-slack vod-without" title={without.full}>{without.short}</span>
  {:else}
    <span class="vod-slack" aria-hidden="true"></span>
  {/if}

  <span class="vod-sub vod-size">{formatBytes(row.size_bytes)}</span>

  <RowActions {row} {onreview} {onpin} {ondelete} {oninspect} {showInspect} />
</article>
