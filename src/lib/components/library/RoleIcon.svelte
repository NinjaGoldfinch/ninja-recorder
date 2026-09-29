<!--
  A lane icon: the client's own, from Community Dragon (`cdragon.rs`).

  **A mask, not a picture.** The file is used for its shape alone and the
  stylesheet fills it, so the badge takes the theme's colour rather than the
  client's gold. That is also why nothing fetched is ever inserted as markup:
  the SVG is only ever a CSS image, the same as a portrait is.

  **No icon, no badge.** Data Dragon has no position art, so there is no second
  source to fall back to, and a drawn stand-in would be a second picture of the
  same role. Until the five files are cached (once, ever), and offline before
  that, the portrait simply has no badge, which is what an unknown role looks
  like too. The row is the same shape either way.

  `role` is one of the five words DEVELOPMENT.md §3.1 names; any other value
  resolves to nothing.
-->

<script lang="ts">
import { positionIcon } from "../../../icons";
import { iconVersion } from "../../stores/icons.svelte";

interface Props {
  role: string | null;
}

const { role }: Props = $props();

// `iconVersion()` is the subscription, as in `Slot`: the lookup is a plain
// read of a cache that grows after the row has rendered.
const src = $derived.by(() => {
  iconVersion();
  return positionIcon(role);
});
</script>

{#if src !== null}
  <span class="vod-role" data-role={role} title={role}>
    <span class="vod-role-glyph" style:--role-icon={`url("${src}")`} aria-hidden="true"></span>
  </span>
{/if}
