<!--
  Retention policy, with the dry run the app itself never offers.

  `set_retention_policy` saves and enforces in one step, deleting files as a
  side effect with nothing shown first. `select_for_deletion` is pure and takes
  an injected clock, so previewing costs nothing &mdash; including at a
  fabricated "now", to check an age rule without waiting days.
-->

<script lang="ts">
import { call, tryCall } from "../../../../dev/ipc";
import type { DevHealth, RetentionPolicy, RetentionPreview } from "../../../../dev/types";
import { devContext } from "../../../dev/context";
import { bytes, timestamp } from "../../../dev/format";
import { devToast } from "../../../stores/devToast.svelte";
import Card from "../Card.svelte";
import DataTable from "../DataTable.svelte";
import Output from "../Output.svelte";
import PanelHead from "../PanelHead.svelte";
import Pill from "../Pill.svelte";

const GIB = 1024 ** 3;
const DAY = 24 * 60 * 60 * 1000;

const ctx = devContext();

let sizeOn = $state(true);
let ageOn = $state(true);
let maxGb = $state(50);
let maxDays = $state(30);
let ageOverrideDays = $state(0);

let preview = $state<RetentionPreview | null>(null);
let previewError = $state<string | null>(null);

const policy = $derived<RetentionPolicy>({
  max_total_bytes: sizeOn ? Math.round(maxGb * GIB) : null,
  max_age_days: ageOn ? maxDays : null,
});

const someAlreadyGone = $derived(preview?.to_delete.some((r) => !r.file_exists) ?? false);

$effect(() => {
  void (async () => {
    const health = await tryCall<DevHealth>("dev_health");
    if (health.ok) {
      const saved = health.value.policy;
      sizeOn = saved.max_total_bytes !== null;
      ageOn = saved.max_age_days !== null;
      if (saved.max_total_bytes !== null) maxGb = Math.round(saved.max_total_bytes / GIB);
      if (saved.max_age_days !== null) maxDays = saved.max_age_days;
    }
    await runPreview();
  })();
});

async function runPreview() {
  const result = await tryCall<RetentionPreview>("dev_retention_preview", {
    policy,
    nowMillis: ageOverrideDays ? Date.now() + ageOverrideDays * DAY : undefined,
  });
  preview = result.ok ? result.value : null;
  previewError = result.ok ? null : result.error;
}

async function apply() {
  await runPreview();
  const count = preview?.to_delete.length ?? 0;
  const ok = await ctx.confirm({
    title: count ? `Delete ${count} recording(s)?` : "Save this policy?",
    body: count
      ? `Enforcement will remove ${count} recording(s) and free about ${bytes(
          preview?.would_free_bytes ?? 0,
        )}. Files are deleted from disk. This cannot be undone.`
      : "Nothing currently matches the policy, so nothing will be deleted.",
    confirmLabel: count ? "Delete them" : "Save",
  });
  if (!ok) return;

  try {
    const report = await call<{ deleted: number[]; freed_bytes: number }>("set_retention_policy", {
      policy,
    });
    devToast(
      `Removed ${report.deleted.length} recording(s), freed ${bytes(report.freed_bytes)}`,
      "ok",
    );
    await runPreview();
  } catch (err) {
    devToast(String(err), "err");
  }
}
</script>

<PanelHead
  title="Retention"
  description="What enforcement would delete, before it deletes it. Age is checked first, then total size; pinned recordings are exempt but still count toward the total."
/>

<Card title="Policy">
  <div class="field-grid">
    <label class="field">
      <span>Max total size (GB)</span>
      <input type="number" min="1" step="1" disabled={!sizeOn} bind:value={maxGb} />
    </label>
    <label class="field">
      <span>Max age (days)</span>
      <input type="number" min="1" step="1" disabled={!ageOn} bind:value={maxDays} />
    </label>
  </div>

  <div class="row" style="margin-top:.6rem">
    <label class="check">
      <input type="checkbox" bind:checked={sizeOn} /> enforce a size limit
    </label>
    <label class="check">
      <input type="checkbox" bind:checked={ageOn} /> enforce an age limit
    </label>
  </div>

  <div class="row" style="margin-top:.9rem">
    <button type="button" onclick={() => void runPreview()}>Preview</button>
    <button type="button" class="danger" onclick={() => void apply()}>Save and enforce</button>
    <span class="spacer"></span>
    <label class="field field-inline">
      <span>Preview as if it were</span>
      <input type="number" style="width:5rem" bind:value={ageOverrideDays} />
      <span class="hint">days from now</span>
    </label>
  </div>

  <p class="hint-block">
    Saving enforces immediately &mdash; that is what the app's own retention form does, and it is
    why a preview exists. The clock override only affects the preview; it lets an age rule be
    checked without waiting for recordings to get old.
  </p>
</Card>

{#if previewError}
  <Card title="Preview"><Output value={previewError} isError /></Card>
{/if}

{#if preview}
  <Card title="Dry run">
    <div class="row" style="margin-bottom:.7rem">
      <Pill label="now" value={bytes(preview.total_bytes)} />
      <Pill label="pinned" value={bytes(preview.pinned_bytes)} />
      <Pill
        label="would delete"
        value={String(preview.to_delete.length)}
        tone={preview.to_delete.length ? "warn" : "ok"}
      />
      <Pill label="would free" value={bytes(preview.would_free_bytes)} />
      <Pill label="after" value={bytes(preview.total_after_bytes)} />
    </div>

    {#if preview.to_delete.length === 0}
      <p class="hint">Nothing would be deleted under this policy.</p>
    {:else}
      <DataTable
        columns={["id", "champion", "started", "size", "pinned", "file"]}
        rows={preview.to_delete.map((r) => [
          r.id,
          r.champion ?? null,
          timestamp(r.started_at),
          bytes(r.size_bytes),
          r.pinned ? "yes" : "no",
          r.file_exists ? "on disk" : "already gone",
        ])}
        numericColumns={new Set(["id"])}
      />
      {#if someAlreadyGone}
        <p class="hint-block">
          Some rows point at files that are already gone, so enforcement will free fewer bytes than
          the estimate above. A rescan would have removed those rows too.
        </p>
      {/if}
    {/if}
  </Card>
{/if}
