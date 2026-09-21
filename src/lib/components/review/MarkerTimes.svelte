<!--
  Both clocks an event has, in the order they are wanted.

  The game clock is the one people say out loud, "Baron at 24:30", and it is
  Live Client Data's own number recorded as the event arrived. The video time
  is the derived half: `game_time_s` mapped through whatever alignment was in
  force, with a fallback for a marker seen before the clock ever moved. So the
  two are not the same fact twice. The game clock is what the event *is*; the
  video time is where this file happens to keep it, and it is the one that can
  be wrong when the alignment was never proven.

  **Both are shown rather than one**, because the pair is also the only place a
  bad alignment is visible: a kill the list calls 24:30 that seeks to black is
  a story the two numbers tell together and neither tells alone.
-->

<script lang="ts">
import { formatTime } from "../../../format";
import type { MarkerRow } from "../../../types";

const { marker }: { marker: MarkerRow } = $props();
</script>

<span class="marker-times">
  <span class="marker-game-time" title="Game clock">{formatTime(marker.game_time_s)}</span>
  <span class="hint" title="Position in the recording">{formatTime(marker.video_time_s)}</span>
</span>
