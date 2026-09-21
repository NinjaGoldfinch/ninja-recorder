<!-- Capture backend: identity, preflight, and a manual start/stop. -->

<script lang="ts">
import { tryCall } from "../../../../dev/ipc";
import { devContext } from "../../../dev/context";
import { bytes } from "../../../dev/format";
import { dev } from "../../../stores/dev.svelte";
import { devToast } from "../../../stores/devToast.svelte";
import Card from "../Card.svelte";
import KeyValues from "../KeyValues.svelte";
import Output from "../Output.svelte";
import PanelHead from "../PanelHead.svelte";

const ctx = devContext();

let lastResult = $state<{ text: string; error: boolean } | null>(null);

const env = $derived(ctx.env);
const health = $derived(dev.health);
const recording = $derived(health?.is_recording ?? false);
const lowSpace = $derived((health?.free_bytes ?? Number.POSITIVE_INFINITY) < 1024 ** 3);

async function run(command: string, describe: (value: unknown) => string) {
  const result = await tryCall<unknown>(command);
  lastResult = result.ok
    ? { text: describe(result.value), error: false }
    : { text: result.error, error: true };
  if (!result.ok) devToast(result.error, "err");
}
</script>

<PanelHead
  title="Recorder"
  description="The capture backend, and a manual start/stop that bypasses the state machine."
/>

<div class="warnbar">
  This starts the recorder <em>directly</em>. The supervisor doesn't know about it, so if the
  state machine also decides to record, the two disagree about whether capture is running &mdash;
  the known divergence documented on <code>Supervisor::start_recording</code>. Use the Simulate
  panel to exercise the real path.
</div>

<Card title="Backend">
  <KeyValues
    pairs={[
      ["Active", env?.recorder_backend ?? "?"],
      ["Platform", env ? `${env.os}/${env.arch}` : "?"],
      ["Capturing", recording ? "yes" : "no"],
      ["Output directory", env?.recordings_dir],
      ["Free space", bytes(health?.free_bytes)],
      [
        "Preflight",
        lowSpace
          ? "would REFUSE — under the 1 GiB minimum"
          : "would allow — at least 1 GiB free",
      ],
    ]}
  />
  {#if env?.recorder_backend === "stub"}
    <p class="hint-block">
      The stub simulates encoder latency and then produces a file: a copy of
      <code>fixtures/sample.mp4</code> when that exists, otherwise a 31-byte placeholder that will
      not decode.
      {#if env.sample_mp4_present}
        It is present, so stub recordings are playable.
      {:else}
        It is not checked in, so stub recordings will not play.
      {/if}
    </p>
  {/if}
</Card>

<Card title="Manual capture">
  <div class="row">
    <button
      type="button"
      class="primary"
      disabled={recording}
      onclick={() => void run("start_recording", () => "Recording started.")}
    >
      Start recording
    </button>
    <button
      type="button"
      class="danger"
      disabled={!recording}
      onclick={() => void run("stop_recording", (path) => `Saved: ${path}`)}
    >
      Stop recording
    </button>
    <button
      type="button"
      class="ghost"
      onclick={() => void run("is_recording", (v) => `is_recording → ${v}`)}
    >
      Query is_recording
    </button>
  </div>
  {#if lastResult}
    <Output value={lastResult.text} isError={lastResult.error} />
  {/if}
</Card>
