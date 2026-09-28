<!--
  The lane matchup: us against the one opponent who played our position.

  **It replaced the ten team portraits**, and the trade is deliberate. Ten
  champions told you who was in the game; one tells you who you actually played
  against, which is the thing a person is reconstructing when they scan a
  library. "The Darius game" is a matchup, not a lobby. The other nine are
  still in `scoreboard_json` for anything that wants them.

  **The opponent is drawn in our shape** (#341): portrait, spells and runes,
  the same three-line stat block, and the same build grid. The pair then
  reads as a comparison, box for box.

  One element spanning four of the row's columns (`vs`, portrait, line,
  build) as a subgrid of it, so its parts line up with the same parts on every
  other row whether or not that row has an opponent.
-->

<script lang="ts">
import { parseScoreboard } from "../../../icons";
import type { RecordingRow } from "../../../types";
import { runesOf } from "../../library/build";
import { laneOpponent } from "../../library/scoreboard";
import Build from "./Build.svelte";
import Perks from "./Perks.svelte";
import Slot from "./Slot.svelte";
import StatLine from "./StatLine.svelte";

const { row }: { row: RecordingRow } = $props();

const them = $derived(laneOpponent(row));
const runes = $derived(runesOf(parseScoreboard(row.scoreboard_json), them));
</script>

{#if them === null}
  <!--
    No opponent is said in words, not drawn as an empty skeleton. A row of
    blank boxes beside a "vs" reads as art that failed to load, which is a bug
    report waiting to happen, where "no matchup" reads as what it is. It spans
    the same columns either way, so nothing after it moves.
  -->
  <span class="vod-versus" data-unknown="true">
    <span class="vod-versus-label" aria-hidden="true">vs</span>
    <span class="vod-sub vod-versus-none">No matchup recorded</span>
  </span>
{:else}
  <span class="vod-versus">
    <span class="vod-versus-label" aria-hidden="true">vs</span>
    <span class="vod-champion">
      <Slot kind="champion" key={them.champion} title={them.champion} extra="vod-portrait" />
      <Perks player={them} {runes} />
    </span>
    <StatLine
      kills={them.kills}
      deaths={them.deaths}
      assists={them.assists}
      cs={them.cs}
      durationS={row.duration_s}
    />
    <Build player={them} position={them.position} />
  </span>
{/if}
