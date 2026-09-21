<!-- A label and a value, in the top bar and beside panel headings. -->

<script lang="ts">
import type { PillTone } from "../../dev/ui-types";

interface Props {
  label: string;
  value: string;
  tone?: PillTone;
  /** Animates, for a state that is actively changing. */
  live?: boolean;
}

const { label, value, tone = "", live = false }: Props = $props();
</script>

<span class="pill" class:pill-ok={tone === "ok"} class:pill-warn={tone === "warn"}
      class:pill-danger={tone === "danger"} class:pill-live={live}>
  {label} <b>{value}</b>
</span>
