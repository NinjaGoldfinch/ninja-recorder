<!--
  Fixture capture and browsing.

  `fixtures.rs` has always written every LCU / Live Client Data response to
  disk when capture is on, but nothing ever read them back: DEVELOPMENT.md §3.3
  asks for a replay mode and none existed. This makes the captured files
  listable, viewable, editable, and hands them to the Simulate panel.
-->

<script lang="ts">
import { call, tryCall } from "../../../../dev/ipc";
import type { FixturesState } from "../../../../dev/types";
import { devContext } from "../../../dev/context";
import { bytes, timestamp } from "../../../dev/format";
import { devToast } from "../../../stores/devToast.svelte";
import Card from "../Card.svelte";
import DataTable from "../DataTable.svelte";
import Output from "../Output.svelte";
import PanelHead from "../PanelHead.svelte";

const ctx = devContext();

let fixturesState = $state<FixturesState | null>(null);
let error = $state<string | null>(null);
let openPath = $state<string | null>(null);
let openContents = $state("");

$effect(() => {
  void reload();
});

async function reload() {
  const result = await tryCall<FixturesState>("dev_fixtures_state");
  fixturesState = result.ok ? result.value : null;
  error = result.ok ? null : result.error;
}

async function setCapture(enabled: boolean) {
  const result = await tryCall<boolean>("dev_set_fixture_recording", { enabled });
  if (!result.ok) {
    devToast(result.error, "err");
    return;
  }
  devToast(result.value ? "Fixture capture on" : "Fixture capture off", "ok");
  await reload();
}

async function openEntry(index: number) {
  const entry = fixturesState?.entries[index];
  if (!entry) return;
  const result = await tryCall<string>("dev_fixture_read", { path: entry.path });
  if (result.ok) {
    openPath = entry.path;
    openContents = result.value;
  } else {
    devToast(result.error, "err");
  }
}

async function revealFolder() {
  const result = await tryCall("dev_open_data_dir", { which: "fixtures" });
  if (!result.ok) devToast(result.error, "err");
}

async function openFile(which: "folder" | "play") {
  if (!openPath) return;
  const result = await tryCall<null>("dev_open_fixture", { path: openPath, which });
  if (!result.ok) devToast(result.error, "err");
}

async function saveCopy() {
  const name = prompt("Save as (name, no extension):");
  if (!name) return;
  const group = prompt("Group (folder):", "live-client");
  if (!group) return;
  try {
    const path = await call<string>("dev_fixture_write", {
      group,
      name,
      contents: openContents,
    });
    devToast(`Saved ${path}`, "ok");
    await reload();
  } catch (err) {
    devToast(String(err), "err");
  }
}
</script>

<PanelHead
  title="Fixtures"
  description="Captured API responses. Turn capture on, play a game, then replay what it recorded through the pipeline."
/>

<Card title="Capture">
  <div class="row">
    <label class="check">
      <input
        type="checkbox"
        checked={fixturesState?.recording_enabled ?? false}
        onchange={(e) => void setCapture((e.currentTarget as HTMLInputElement).checked)}
      />
      Record every LCU and Live Client Data response
    </label>
    <span class="spacer"></span>
    <button type="button" class="ghost tiny" onclick={() => void revealFolder()}>
      Reveal capture folder
    </button>
    <button type="button" class="ghost tiny" onclick={() => void reload()}>Reload list</button>
  </div>

  {#if fixturesState}
    <dl class="kv" style="margin-top:.7rem">
      <dt>Capture folder</dt>
      <dd>{fixturesState.capture_dir ?? "—"}</dd>
      <dt>Repo fixtures</dt>
      <dd>{fixturesState.repo_dir ?? "not resolvable in this build"}</dd>
    </dl>
  {/if}

  <p class="hint-block">
    Capture used to be settable only by launching with
    <code>NINJA_RECORDER_RECORD_FIXTURES</code> set, which meant deciding before the app started.
    Writes land under the capture folder as <code>&lt;group&gt;/&lt;endpoint&gt;.json</code>, one
    file per endpoint &mdash; a later response overwrites an earlier one.
  </p>
</Card>

{#if error}
  <Card title="Error"><Output value={error} isError /></Card>
{/if}

<Card title="Files">
  {#if fixturesState && fixturesState.entries.length}
    <DataTable
      columns={["source", "group", "name", "size", "modified"]}
      rows={fixturesState.entries.map((f) => [
        f.source,
        f.group || "—",
        f.name,
        bytes(f.bytes),
        f.modified_millis ? timestamp(f.modified_millis) : null,
      ])}
      onrow={(i) => void openEntry(i)}
      emptyMessage="No fixtures found."
    />
  {:else}
    <p class="hint">
      No fixtures found. Turn capture on and run the app against a live client, or check in JSON
      under the repo's <code>fixtures/</code> directory.
    </p>
  {/if}
</Card>

{#if openPath}
  <Card title={openPath} raw>
    <textarea rows="16" spellcheck="false" bind:value={openContents}></textarea>
    <div class="row wrap" style="margin-top:.6rem">
      <button type="button" class="primary" onclick={() => ctx.navigate("simulate", openContents)}>
        Send to the snapshot injector
      </button>
      <button type="button" onclick={() => void saveCopy()}>Save a copy&hellip;</button>
      <button type="button" class="ghost" onclick={() => void openFile("play")}>
        Open in editor
      </button>
      <button type="button" class="ghost" onclick={() => void openFile("folder")}>
        Show in folder
      </button>
      <button type="button" class="ghost" onclick={() => (openPath = null)}>Close</button>
    </div>
    <p class="hint-block">
      The textarea is fine for a small payload and useless for a large one &mdash; the captured
      <code>eog-stats-block</code> is 99 KB of nested JSON. "Open in editor" hands it to whatever
      the OS opens <code>.json</code> with, which has folding and search.
    </p>
    <p class="hint-block">
      Saving writes into the capture folder, never over a repo fixture &mdash; checked-in fixtures
      are test inputs and should change through git, not through this page.
    </p>
  </Card>
{/if}
