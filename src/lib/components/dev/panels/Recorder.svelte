<!--
  The daemon's capture backend: identity, what it is writing, and preflight.

  **All of it is the daemon's** (#282). `dev_health` runs in the process that
  owns the `Recorder`; `dev_env_info` runs in the UI, whose recorder refuses
  everything, and this panel used to read its backend from there.

  There is no manual start or stop. `start_recording` and `stop_recording`
  reach the daemon's recorder around the supervisor, and both real backends
  record the game window, so without a game a start fails, and during one it
  either collides with the supervisor's recording or ends it under the
  finalize. The path that exercises capture is Simulate, through the state
  machine. docs/dev-portal.md, "The Recorder panel has no manual controls".
-->

<script lang="ts">
import { devContext } from "../../../dev/context";
import { bytes, workerState } from "../../../dev/format";
import { dev } from "../../../stores/dev.svelte";
import Card from "../Card.svelte";
import KeyValues from "../KeyValues.svelte";
import PanelHead from "../PanelHead.svelte";

const ctx = devContext();

const env = $derived(ctx.env);
const health = $derived(dev.health);
const recorder = $derived(health?.recorder ?? null);
const lowSpace = $derived((health?.free_bytes ?? Number.POSITIVE_INFINITY) < 1024 ** 3);
</script>

<PanelHead
  title="Recorder"
  description="The daemon's capture backend: which one is live, whether it is capturing, and into what file."
/>

{#if !health}
  <p class="hint">
    {dev.healthError
      ? `The daemon did not answer: ${dev.healthError}`
      : "Waiting for the first health poll…"}
  </p>
{:else}
  <Card title="Backend (daemon)">
    <KeyValues
      pairs={[
        ["Live backend", recorder?.backend],
        ["Setting", recorder?.configured],
        ["Capturing", health.is_recording ? "yes" : "no"],
        ["Current file", recorder?.current_file],
        ["Capture worker", workerState(recorder?.worker_running)],
        ["Supervisor state", health.supervisor.state],
        ["Output directory", env?.recordings_dir],
        ["Free space", bytes(health.free_bytes)],
        [
          "Preflight",
          lowSpace
            ? "would REFUSE — under the 1 GiB minimum"
            : "would allow — at least 1 GiB free",
        ],
      ]}
    />
    {#if recorder?.backend === "stub"}
      <p class="hint-block">
        The stub simulates encoder latency and then produces a file: a copy of
        <code>fixtures/sample.mp4</code> when that exists, otherwise a 31-byte placeholder that
        will not decode.
        {#if env?.sample_mp4_present}
          It is present, so stub recordings are playable.
        {:else}
          It is not checked in, so stub recordings will not play.
        {/if}
      </p>
    {/if}
  </Card>

  <Card title="Starting and stopping">
    <p class="hint-block">
      The supervisor decides when to record, so a capture started here would be one it does not
      know about: with no game the backend has no window to capture, and during one it would
      collide with the recording the state machine already started. Drive the state machine from
      Simulate instead: the daemon's recorder is then started and stopped by the path a real game
      takes, and the recording is finalized into a row.
    </p>
    <div class="row">
      <button type="button" class="primary" onclick={() => ctx.navigate("simulate")}>
        Open Simulate
      </button>
    </div>
  </Card>
{/if}
