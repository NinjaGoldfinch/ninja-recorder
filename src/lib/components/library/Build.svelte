<!--
  A player's build: a 4×2 grid, the trinket always top right.

  `buildBoxes` decides which item goes in which box, including the one rule
  worth knowing here: an eighth box exists only for a bot laner, for the boots
  the role quest moves out of the inventory. Every other role's grid simply
  stops at seven, because its role slot holds a quest token rather than an
  item and an empty box would read as a missing one.

  **Empty boxes are rendered, not skipped.** A build with four items is a
  different thing from a game with no scoreboard, and a grid that shrank to
  fit would say neither. The boxes are the shape of the information.
-->

<script lang="ts">
import type { ScoreboardPlayer } from "../../../types";
import { buildBoxes, TRINKET_BOX } from "../../library/build";
import Slot from "./Slot.svelte";

interface Props {
  player: ScoreboardPlayer | null;
  /** The player's own position; see `buildBoxes`. */
  position: string | null | undefined;
}

const { player, position }: Props = $props();

const boxes = $derived(buildBoxes(player, position));
</script>

<span class="vod-items" aria-hidden="true">
  {#each boxes as id, i (i)}
    {#if id === null}
      <Slot extra={i === TRINKET_BOX ? "vod-trinket" : ""} />
    {:else}
      <Slot kind="item" key={id} title={`Item ${id}`} extra={i === TRINKET_BOX ? "vod-trinket" : ""} />
    {/if}
  {/each}
</span>
