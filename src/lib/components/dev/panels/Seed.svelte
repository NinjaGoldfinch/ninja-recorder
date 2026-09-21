<!--
  Synthetic library generation.

  The presets and the spec live in `lib/dev/seed.ts`; what is here is the form
  and the two commands it drives.
-->

<script lang="ts">
import { call, tryCall } from "../../../../dev/ipc";
import type { SeedReport } from "../../../../dev/types";
import { devContext } from "../../../dev/context";
import { bytes } from "../../../dev/format";
import {
  type BooleanKey,
  type NumericKey,
  PRESETS,
  type Preset,
  type SeedForm,
  toForm,
  toSpec,
} from "../../../dev/seed";
import { devToast } from "../../../stores/devToast.svelte";
import Card from "../Card.svelte";
import Output from "../Output.svelte";
import PanelHead from "../PanelHead.svelte";

const ctx = devContext();

let active = $state<Preset>(PRESETS[0]);
let form = $state<SeedForm>(toForm(PRESETS[0].spec));
let report = $state<{ value: unknown; error: boolean } | null>(null);
let busy = $state(false);

const spec = $derived(toSpec(form, active.spec));

/** Every numeric field, as `[key, label, help]`. A list rather than markup so
 *  the nine of them cannot drift apart in spacing or wording. */
const NUMBER_FIELDS: Array<[NumericKey, string, string]> = [
  ["count", "Recordings", ""],
  ["seed", "Random seed", "Same seed, same library"],
  ["spread_days", "Spread over days", "Back-dates started_at"],
  ["pinned_every", "Pin every Nth", "0 pins nothing"],
  ["duration_min_s", "Min duration (s)", ""],
  ["duration_max_s", "Max duration (s)", ""],
  ["markers_min", "Min markers", ""],
  ["markers_max", "Max markers", ""],
  ["file_bytes", "File size (bytes)", "Sparse — costs no real disk"],
];

const CHECK_FIELDS: Array<[BooleanKey, string]> = [
  ["samples", "Generate the 1 Hz advantage curve"],
  ["use_sample_mp4", "Copy fixtures/sample.mp4 when present"],
  ["messy", "Mix in NULL metadata, unicode, and long names"],
];

function choose(preset: Preset) {
  active = preset;
  form = toForm(preset.spec);
  report = null;
}

async function seed() {
  busy = true;
  const result = await tryCall<SeedReport>("dev_seed_library", { spec });
  busy = false;

  if (!result.ok) {
    report = { value: result.error, error: true };
    devToast(result.error, "err");
    return;
  }

  const r = result.value;
  report = {
    value: {
      recordings: r.recording_ids.length,
      ids: r.recording_ids,
      markers: r.markers_inserted,
      samples: r.samples_inserted,
      bytes_on_disk: bytes(r.bytes_written),
      playable: r.used_sample_mp4,
      first_path: r.paths[0] ?? null,
    },
    error: false,
  };
  devToast(`Seeded ${r.recording_ids.length} recording(s)`, "ok");
}

async function clearSeeded() {
  const ok = await ctx.confirm({
    title: "Clear seeded recordings?",
    body: "Every seed-*.mp4 file and its database row will be deleted. Captured recordings are not touched.",
    confirmLabel: "Clear seeded",
  });
  if (!ok) return;

  try {
    const r = await call<{ rows_deleted: number; files_deleted: number }>("dev_clear_seeded");
    devToast(`Removed ${r.rows_deleted} row(s), ${r.files_deleted} file(s)`, "ok");
    report = null;
  } catch (err) {
    devToast(String(err), "err");
  }
}
</script>

<PanelHead
  title="Seed"
  description="Generate a realistic library from nothing — real files, real rows, real markers and samples."
/>

{#if ctx.env && !ctx.env.sample_mp4_present}
  <div class="warnbar">
    <code>fixtures/sample.mp4</code> is not present, so seeded files are sparse placeholders that no
    demuxer will open. Everything except video playback works; drop a short real clip there for the
    Review-ready preset to be worth running.
  </div>
{/if}

<Card title="Presets">
  <div class="row">
    {#each PRESETS as preset (preset.id)}
      <button
        type="button"
        class:primary={preset.id === active.id}
        onclick={() => choose(preset)}
      >
        {preset.label}
      </button>
    {/each}
  </div>
  <p class="hint-block">{active.what}</p>
</Card>

<Card title="Specification">
  <div class="field-grid">
    <!-- The help text sits inside the label: as a sibling it would take its
         own `.field-grid` cell and land beside an unrelated field. -->
    {#each NUMBER_FIELDS as [key, label, help] (key)}
      <label class="field">
        <span>{label}</span>
        <input type="number" bind:value={form[key]} />
        {#if help}<span class="hint">{help}</span>{/if}
      </label>
    {/each}
  </div>

  <div class="row" style="margin-top:.8rem">
    {#each CHECK_FIELDS as [key, label] (key)}
      <label class="check">
        <input type="checkbox" bind:checked={form[key]} />
        {label}
      </label>
    {/each}
  </div>

  <div class="row" style="margin-top:1rem">
    <button type="button" class="primary" disabled={busy} onclick={() => void seed()}>
      {busy ? "Seeding…" : `Seed ${spec.count} recording(s)`}
    </button>
    <span class="hint">writes {bytes(spec.count * spec.file_bytes)} (sparse)</span>
  </div>

  {#if report}
    <Output value={report.value} isError={report.error} />
  {/if}
</Card>

<Card title="Cleanup" extraClass="card-danger">
  <p class="hint-block" style="margin-top:0">
    Seeded recordings are named <code>seed-*.mp4</code>. Clearing removes only those rows and files
    — anything captured for real is left alone.
  </p>
  <div class="row" style="margin-top:.7rem">
    <button type="button" class="danger" onclick={() => void clearSeeded()}>
      Clear seeded recordings
    </button>
  </div>
</Card>
