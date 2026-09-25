<!--
  The spells, runes and items the game ended on.

  Order is load-bearing: the grid fills by column, so the four perk slots land
  as spell 1, spell 2 | keystone, secondary tree.

  **A game with no rune page draws no rune slots** (#281). Augment modes such
  as ARAM Mayhem have none, and both writers then leave `our_runes` out, so
  two empty frames would stand for a page that never existed rather than one
  that is missing. The perks column is a fixed track in `.vod-row`, so the
  columns beside it do not move.
-->

<script lang="ts">
import { parseScoreboard } from "../../../icons";
import type { RecordingRow } from "../../../types";
import Slot from "./Slot.svelte";

const { row }: { row: RecordingRow } = $props();

const board = $derived(parseScoreboard(row.scoreboard_json));
const us = $derived(board?.players.find((p) => p.is_us) ?? null);
const runes = $derived(board?.our_runes ?? null);

type SlotSpec = {
  kind: "spell" | "spell-id" | "rune" | "item" | null;
  key: string | number;
  title: string;
};
const EMPTY: SlotSpec = { kind: null, key: "", title: "" };

/** Pads to `length` with empty boxes, and never renders more than that. */
function fixed(slots: SlotSpec[], length: number): SlotSpec[] {
  return [...slots.slice(0, length), ...Array(Math.max(0, length - slots.length)).fill(EMPTY)];
}

// Names when the scoreboard was captured live, ids when it was rebuilt from
// match history. Both find the art; neither is converted into the other,
// because that would need the CDN in a path that only talks to the League
// client.
const spells = $derived(
  fixed(
    us === null
      ? []
      : us.spells.length > 0
        ? us.spells.map((s) => ({ kind: "spell" as const, key: s, title: s }))
        : (us.spell_ids ?? []).map((id) => ({
            kind: "spell-id" as const,
            key: id,
            title: `Spell ${id}`,
          })),
    2,
  ),
);

const perks = $derived(
  runes === null
    ? []
    : [
        {
          kind: "rune" as const,
          key: runes.keystone_id,
          title: runes.keystone || "Keystone",
        },
        { kind: "rune" as const, key: runes.secondary_tree_id, title: "Secondary tree" },
      ],
);

// Six plus the trinket, which is what an inventory holds.
const items = $derived(
  fixed(
    (us?.items ?? []).map((id) => ({ kind: "item" as const, key: id, title: `Item ${id}` })),
    7,
  ),
);
</script>

<span class="vod-perks" aria-hidden="true">
  {#each [...spells, ...perks] as slot, i (i)}
    <Slot kind={slot.kind} key={slot.key} title={slot.title} />
  {/each}
</span>
<span class="vod-items" aria-hidden="true">
  {#each items as slot, i (i)}
    <Slot kind={slot.kind} key={slot.key} title={slot.title} />
  {/each}
</span>
