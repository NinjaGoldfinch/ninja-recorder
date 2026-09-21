<!--
  Any value, pretty-printed.

  **Text, never markup.** This renders command results, database rows and error
  strings, all of which are backend output that nothing in this frontend wrote.
-->

<script lang="ts">
interface Props {
  value: unknown;
  isError?: boolean;
}

const { value, isError = false }: Props = $props();

/**
 * **`JSON.stringify` throws rather than returning undefined** on a circular
 * structure, so `?? String(value)` never caught it: `ui.ts` had the same shape
 * and a cyclic value would take the whole panel out. This renders arbitrary
 * backend output, so it has to survive whatever it is handed.
 */
const text = $derived.by(() => {
  if (typeof value === "string") return value;
  try {
    return JSON.stringify(value, null, 2) ?? String(value);
  } catch {
    return String(value);
  }
});
</script>

<pre class="output" class:output-error={isError}>{text}</pre>
