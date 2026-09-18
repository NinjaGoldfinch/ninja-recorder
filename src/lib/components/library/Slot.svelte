<!--
  One art slot: a fixed box that may or may not have a picture in it yet.

  **Empty slots are rendered, not skipped.** A build with four items is a
  different thing from a game with no scoreboard, and a row that shrank to fit
  would say neither. The boxes are the shape of the information.

  Every slot starts blank and fills in when the CDN answers, so a row renders
  identically offline, just without pictures. The `title` carries what each one
  is, which is the whole of what a row with no art can tell you.
-->

<script lang="ts">
import { championIcon, itemIcon, runeIcon, spellIcon, spellIconById } from "../../../icons";
import { iconVersion } from "../../stores/icons.svelte";

interface Props {
  /** Which lookup to use. `null` renders the empty box and asks for nothing. */
  kind?: "champion" | "item" | "spell" | "spell-id" | "rune" | null;
  key?: string | number;
  title?: string;
  /** Extra class on the box, for the few slots the stylesheet sizes
   *  differently (`vod-versus-portrait`). */
  extra?: string;
}

const { kind = null, key = "", title = "", extra = "" }: Props = $props();

// `iconVersion()` is read first and its value discarded: it is the
// subscription. The lookups themselves are synchronous reads of a cache
// that `fillInArt` grows, and nothing else would tell this component the
// answer had changed.
const src = $derived.by(() => {
  iconVersion();
  switch (kind) {
    case "champion":
      return championIcon(String(key));
    case "item":
      return itemIcon(Number(key));
    case "spell":
      return spellIcon(String(key));
    case "spell-id":
      return spellIconById(Number(key));
    case "rune":
      return runeIcon(Number(key));
    default:
      return null;
  }
});
</script>

{#if kind === null}
  <span class="vod-slot vod-slot-empty {extra}"></span>
{:else}
  <span class="vod-slot {extra}" {title}>
    {#if src}
      <img {src} alt="" loading="lazy" />
    {/if}
  </span>
{/if}
