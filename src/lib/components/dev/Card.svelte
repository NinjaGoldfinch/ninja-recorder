<!--
  A titled box. The portal's basic unit of layout.
-->

<script lang="ts">
import type { Snippet } from "svelte";

interface Props {
  /** Omitted for a box that needs no heading. */
  title?: string;
  /**
   * For titles that are literal system strings: a path, a command name.
   * The default treatment uppercases, which mangles them.
   */
  raw?: boolean;
  extraClass?: string;
  children: Snippet;
}

const { title = "", raw = false, extraClass = "", children }: Props = $props();
</script>

<section class="card {extraClass}">
  {#if title}
    <h2 class:raw>{title}</h2>
  {/if}
  {@render children()}
</section>
