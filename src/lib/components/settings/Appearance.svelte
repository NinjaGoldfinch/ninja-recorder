<!-- Theme and default sort. -->

<script lang="ts">
import type { SortKey, ThemePref } from "../../../prefs";
import { setThemePref } from "../../../theme";
import { applyDefaultSort } from "../../stores/library.svelte";
import { setPref, settings } from "../../stores/settings.svelte";
import SettingRow from "./SettingRow.svelte";

const THEMES: { value: ThemePref; label: string }[] = [
  { value: "system", label: "System" },
  { value: "light", label: "Light" },
  { value: "dark", label: "Dark" },
];

const SORTS: { value: SortKey; label: string }[] = [
  { value: "newest", label: "Newest first" },
  { value: "oldest", label: "Oldest first" },
  { value: "longest", label: "Longest first" },
  { value: "champion", label: "Champion (A–Z)" },
];

function chooseTheme(value: ThemePref) {
  // `theme.ts` owns `html[data-theme]` and the matchMedia listener that makes
  // "System" follow the OS live. This asks it to change; it does not write
  // the attribute itself, and **must not**, or the listener would be writing
  // against a second owner.
  setThemePref(value);
  settings.prefs.theme = value;
}

function chooseSort(value: SortKey) {
  setPref("defaultSort", value);
  // The library reads its sort from the store, so the preference and the
  // control the user is looking at in the other view stay in step.
  applyDefaultSort(value);
}
</script>

<section class="settings-group">
  <h3>Appearance</h3>

  <SettingRow label="Theme" hint={"“System” follows your OS setting as it changes."}>
    <div class="segmented" role="radiogroup" aria-label="Theme">
      {#each THEMES as choice (choice.value)}
        <button
          type="button"
          role="radio"
          aria-checked={settings.prefs.theme === choice.value}
          onclick={() => chooseTheme(choice.value)}>{choice.label}</button
        >
      {/each}
    </div>
  </SettingRow>

  <SettingRow label="Default sort" hint="Applied when the app opens.">
    <select
      aria-label="Default sort"
      value={settings.prefs.defaultSort}
      onchange={(e) => chooseSort((e.currentTarget as HTMLSelectElement).value as SortKey)}
    >
      {#each SORTS as sort (sort.value)}
        <option value={sort.value}>{sort.label}</option>
      {/each}
    </select>
  </SettingRow>
</section>
