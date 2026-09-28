<!--
  A player's summoner spells and rune page, as a 2×2 block beside the portrait.

  Order is load-bearing: the grid fills by column, so the four slots land as
  spell 1, spell 2 | keystone, secondary tree. Spells are square and runes
  round, which is how the game draws each.

  **A game with no rune page draws no rune slots** (#281). Augment modes such
  as ARAM Mayhem have none, so two empty frames would stand for a page that
  never existed rather than one that is missing. The column is `auto`, and the
  shared grid keeps every other column where it is.

  Used for both players, which is the point: the opponent's block is ours,
  with their spells and their page (#341).
-->

<script lang="ts">
import type { ScoreboardPlayer, ScoreboardRunes } from "../../../types";
import Slot from "./Slot.svelte";

interface Props {
  player: ScoreboardPlayer | null;
  runes: ScoreboardRunes | null;
}

const { player, runes }: Props = $props();

type SlotSpec = {
  kind: "spell" | "spell-id" | "rune" | null;
  key: string | number;
  title: string;
  extra: string;
};

// Names when the scoreboard was captured live, ids when it was rebuilt from
// match history. Both find the art; neither is converted into the other,
// because that would need the CDN in a path that only talks to the League
// client.
const spells = $derived.by((): SlotSpec[] => {
  const named =
    player === null
      ? []
      : player.spells.length > 0
        ? player.spells.map((s) => ({
            kind: "spell" as const,
            key: s,
            title: s,
            extra: "vod-spell",
          }))
        : (player.spell_ids ?? []).map((id) => ({
            kind: "spell-id" as const,
            key: id,
            title: `Spell ${id}`,
            extra: "vod-spell",
          }));
  const empty: SlotSpec = { kind: null, key: "", title: "", extra: "vod-spell" };
  return [...named.slice(0, 2), ...Array(Math.max(0, 2 - named.length)).fill(empty)];
});

const page = $derived(
  runes === null
    ? []
    : [
        {
          kind: "rune" as const,
          key: runes.keystone_id,
          title: runes.keystone || "Keystone",
          extra: "vod-rune",
        },
        {
          kind: "rune" as const,
          key: runes.secondary_tree_id,
          title: "Secondary tree",
          extra: "vod-rune",
        },
      ],
);
</script>

<span class="vod-perks" aria-hidden="true">
  {#each [...spells, ...page] as slot, i (i)}
    <Slot kind={slot.kind} key={slot.key} title={slot.title} extra={slot.extra} />
  {/each}
</span>
