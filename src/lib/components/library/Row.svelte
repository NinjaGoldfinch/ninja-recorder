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
  formatRelative,
  lpLabel,
  patchLabel,
  queueOrModeLabel,
  rankLabel,
  vodHeading,
  vodTitle,
} from "../../../format";
import { parseScoreboard } from "../../../icons";
import type { RecordingRow } from "../../../types";
import { runesOf } from "../../library/build";
import { recordedWithout } from "../../library/problems";
import { laneOpponent, outcomeAttr, selfPlayer } from "../../library/scoreboard";
import Build from "./Build.svelte";
import Matchup from "./Matchup.svelte";
import Perks from "./Perks.svelte";
import RowActions from "./RowActions.svelte";
import Slot from "./Slot.svelte";
import StatLine from "./StatLine.svelte";

interface Props {
  row: RecordingRow;
  onopen: (row: RecordingRow) => void;
  onpin: (row: RecordingRow) => void;
  ondelete: (row: RecordingRow) => void;
  oninspect: (row: RecordingRow) => void;
  showInspect: boolean;
}

const { row, onopen, onpin, ondelete, oninspect, showInspect }: Props = $props();

const title = $derived(vodTitle(row));
const queue = $derived(queueOrModeLabel(row));
const patch = $derived(patchLabel(row.patch));
const rank = $derived(rankLabel(row.tier, row.division));
// Only beside a rank. LP with no tier is a number with no scale, and the
// column can be in that state: `fill_ranked` writes all three together, but
// a row from before migration 10 has neither.
const lp = $derived(rank === null ? null : lpLabel(row.lp_after));
const length = $derived(row.duration_s === null ? null : formatClock(row.duration_s));

const us = $derived(selfPlayer(row));
const ourRunes = $derived(runesOf(parseScoreboard(row.scoreboard_json), us));
// The scoreboard's own position first; the recording's `role` where a board
// predates positions. It decides whether our build has a boots box.
const ourPosition = $derived(us?.position ?? row.role);

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
// otherwise only the file size, so a row that says it is still the same shape
// as one that does not.
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
  <!--
    What the game was and how it went, in four short lines rather than four
    columns: none of them is a number worth comparing down the list. They
    answer "is this the game I mean", which is read once per row. The rank
    sits here, beside the queue it belongs to (#341).
  -->
  <span class="vod-meta">
    <span class="vod-line">
      {#if queue === null}
        <span class="vod-queue vod-missing">&mdash;</span>
      {:else}
        <span class="vod-queue">{queue}</span>
      {/if}{#if patch !== null}<span class="vod-sub">&nbsp;&middot; {patch}</span>{/if}
    </span>
    <span class="vod-rank">
      {#if rank === null}
        <span class="vod-missing">&mdash;</span>
      {:else}
        <span title="The rank this game was played at">{rank}</span>{#if lp !== null}&nbsp;&middot;
          {lp}{/if}
      {/if}
    </span>
    <span class="vod-sub">
      <span class="vod-champ" {title}>{title}</span> &middot;
      {#if row.role === null}
        <span class="vod-missing">Unknown</span>
      {:else}{row.role}{/if}
    </span>
    <span class="vod-sub">
      {#if length === null}
        <span class="vod-missing">&mdash;</span>
      {:else}{length}{/if}{#if outcomeWord}&nbsp;&middot;
        <span class="vod-outcome">{outcomeWord}</span>
      {/if}
      &middot;
      <time datetime={new Date(row.started_at).toISOString()} title={formatDateTime(row.started_at)}
        >{formatRelative(row.started_at)}</time
      >
    </span>
  </span>

  <span class="vod-champion">
    <Slot kind="champion" key={row.champion ?? ""} extra="vod-portrait" />
    <Perks player={us} runes={ourRunes} />
  </span>

  <StatLine kills={row.kda_k} deaths={row.kda_d} assists={row.kda_a} cs={row.cs} durationS={row.duration_s} />

  <Build player={us} position={ourPosition} />

  <Matchup {row} />

  <span class="vod-slack">
    {#if without}
      <span class="vod-without" title={without.full}>{without.short}</span>
    {/if}
    <span class="vod-sub vod-size">{formatBytes(row.size_bytes)}</span>
  </span>

  <RowActions {row} {onpin} {ondelete} {oninspect} {showInspect} />
</article>
