<!--
  One rating: a segmented control whose lit answer wears its tone (green,
  amber or red, as in the spreadsheet this replaces). Clicking the lit answer
  again unsets it, because "not rated yet" is a real answer.
-->

<script lang="ts" generics="T extends string">
import { type Choice, toggle } from "../../reviewform/ratings";

interface Props {
  label: string;
  choices: Choice<T>[];
  value: T | null;
  onchange: (next: T | null) => void;
}

const { label, choices, value, onchange }: Props = $props();
</script>

<div class="setting-row">
  <span class="setting-label">{label}</span>
  <div class="segmented rating" role="radiogroup" aria-label={label}>
    {#each choices as choice (choice.value)}
      <button
        type="button"
        role="radio"
        aria-checked={value === choice.value}
        data-tone={choice.tone}
        onclick={() => onchange(toggle(value, choice.value))}>{choice.label}</button
      >
    {/each}
  </div>
</div>
