<!--
  A lane icon, drawn here rather than fetched.

  **Data Dragon has no position art**, and `ddragon.rs` is deliberately the
  only place art comes from: Community Dragon was a source once and was taken
  out. Five shapes on a 136-unit grid (the size of the client's own icons)
  cost less than a second source would, and they draw offline, which is the
  case a local VOD library is usually in.

  **Drawn for twelve pixels, not for the grid.** The badge is about that size
  at DPR 1, where the client's own details (the dimmed map around each lane,
  and gaps under a pixel wide) antialias into a grey smudge. So only the lit
  part is drawn, which is still enough to tell top from bottom (the corner
  moves), the jungle claw is bolder than the client's, and each shape carries
  its own `box` so it fills the badge without a transform that would move its
  edges off the pixel grid again.

  `role` is one of the five words DEVELOPMENT.md §3.1 names; any
  other value draws nothing, which is how the row says the role is unknown.
-->

<script lang="ts">
interface Props {
  role: string | null;
}

const { role }: Props = $props();

type Shape = { d: string; box: string; flip?: boolean };

const CORNER = "M16 16H112L93 35H35V93L16 112Z";

const SHAPES: Record<string, Shape> = {
  Top: { d: CORNER, box: "8 8 120 120" },
  Bottom: { d: CORNER, box: "8 8 120 120", flip: true },
  Middle: { d: "M98 16H120V37L37 120H16V99Z", box: "8 8 120 120" },
  Jungle: {
    d:
      "M30 8Q64 44 66 84Q70 66 84 56Q78 92 70 128Q46 106 26 92Q22 66 4 44Q32 48 40 70Q42 38 30 8Z" +
      "M108 6Q104 44 84 84Q72 62 76 52Q96 34 108 6Z" +
      "M132 40Q102 50 94 76Q90 98 86 112Q104 102 110 90Q112 58 132 40Z",
    box: "0 0 136 136",
  },
  Support: {
    d:
      "M57 13H84L88 20L69 43L49 20Z" +
      "M58 53L64 51L69 56L74 51L80 53L86 114L69 123L52 114Z" +
      "M1 37H44L56 52L49 58L45 54L38 70L25 66L31 52Q12 50 1 37Z" +
      "M135 37H92L80 52L87 58L91 54L98 70L111 66L105 52Q124 50 135 37Z",
    box: "-4 -4 144 144",
  },
};

const shape = $derived(role === null ? null : (SHAPES[role] ?? null));
</script>

{#if shape !== null}
  <span class="vod-role" data-role={role} title={role}>
    <svg viewBox={shape.box} aria-hidden="true">
      <path d={shape.d} transform={shape.flip ? "rotate(180 68 68)" : undefined} />
    </svg>
  </span>
{/if}
