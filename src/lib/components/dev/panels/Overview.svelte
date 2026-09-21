<!-- Health dashboard: state machine, recorder, LCU, disk, paths. -->

<script lang="ts">
import { tryCall } from "../../../../dev/ipc";
import { GAME_STATES, type LcuStatus } from "../../../../dev/types";
import { devContext } from "../../../dev/context";
import { bytes, duration, timestamp } from "../../../dev/format";
import { dev } from "../../../stores/dev.svelte";
import { devToast } from "../../../stores/devToast.svelte";
import Card from "../Card.svelte";
import KeyValues from "../KeyValues.svelte";
import Output from "../Output.svelte";
import PanelHead from "../PanelHead.svelte";

const ctx = devContext();

let lcu = $state<LcuStatus | null>(null);
let lcuError = $state<string | null>(null);

const health = $derived(dev.health);
const env = $derived(ctx.env);
const session = $derived(health?.session ?? null);

// **A separate, slower call.** It does lockfile discovery plus two HTTP
// round trips, so it stays out of the 1 Hz health poll and refreshes only
// when this panel mounts.
$effect(() => {
  void tryCall<LcuStatus>("lcu_status").then((result) => {
    lcu = result.ok ? result.value : null;
    lcuError = result.ok ? null : result.error;
  });
});

async function reveal(which: string) {
  const result = await tryCall("dev_open_data_dir", { which });
  if (!result.ok) devToast(result.error, "err");
}
</script>

<PanelHead
  title="Overview"
  description="Live state of every subsystem. Polled once a second while the top bar's Live toggle is on."
/>

{#if !health}
  <p class="hint">Waiting for the first health poll&hellip;</p>
{:else}
  <!-- DEVELOPMENT.md §3.4's diagram, with the live state lit. -->
  <Card title="Game state machine">
    <div class="states">
      {#each GAME_STATES as state, i (state)}
        {#if i > 0}<span class="state-arrow">&rarr;</span>{/if}
        <span class="state-node" class:current={state === health.supervisor.state}>{state}</span>
      {/each}
    </div>
  </Card>

  <div class="card-grid">
    <Card title="Recorder">
      <KeyValues
        pairs={[
          ["Backend", env?.recorder_backend ?? "?"],
          ["Capturing", health.is_recording ? "yes" : "no"],
          ["Free space", bytes(health.free_bytes)],
        ]}
      />
    </Card>

    <Card title="Library">
      <KeyValues
        pairs={[
          ["Recordings", String(health.counts.recordings)],
          ["Markers", String(health.counts.markers)],
          ["Samples", String(health.counts.samples)],
          ["Total size", bytes(health.total_bytes)],
        ]}
      />
    </Card>

    <Card title="Retention policy">
      <KeyValues
        pairs={[
          [
            "Max total",
            health.policy.max_total_bytes === null
              ? "unbounded"
              : bytes(health.policy.max_total_bytes),
          ],
          [
            "Max age",
            health.policy.max_age_days === null
              ? "unbounded"
              : `${health.policy.max_age_days} days`,
          ],
        ]}
      />
    </Card>

    <Card title="Live session">
      {#if !session}
        <p class="hint">
          No recording session open. Markers and samples are only collected between
          <code>Recorder::start</code> and <code>Recorder::stop</code>.
        </p>
      {:else}
        <KeyValues
          pairs={[
            ["Markers", String(session.marker_count)],
            ["Samples", String(session.sample_count)],
            ["Elapsed", duration(session.elapsed_s)],
            [
              "Time alignment",
              session.alignment_offset_s === null
                ? "not aligned yet — no snapshot has arrived"
                : `${session.alignment_offset_s.toFixed(2)}s offset`,
            ],
            ["Started", timestamp(session.started_at_millis)],
          ]}
        />
        {#if session.recent_markers.length}
          <p class="hint" style="margin-top:.6rem">
            Latest: {session.recent_markers
              .slice(0, 6)
              .map((m) => `${m.kind}@${m.game_time_s.toFixed(0)}s`)
              .join(", ")}
          </p>
        {/if}
      {/if}
    </Card>

    <Card title="League Client">
      {#if lcuError}
        <Output value={lcuError} isError />
      {:else if !lcu}
        <p class="hint">Not checked yet.</p>
      {:else if lcu.error}
        <Output value={lcu.error} isError />
      {:else if !lcu.connected}
        <p class="hint">
          Not running &mdash; no lockfile found. Set
          <code>NINJA_RECORDER_LOCKFILE_PATH</code>, or use the Simulate panel to drive the state
          machine without a client.
        </p>
      {:else}
        <KeyValues pairs={[["Summoner", lcu.summoner], ["Phase", lcu.phase]]} />
      {/if}
    </Card>
  </div>

  {#if health.supervisor.last_finalized}
    <Card title="Last finalized recording">
      <KeyValues
        pairs={[
          [
            "DB row",
            health.supervisor.last_finalized.recording_id === null
              ? "WRITE FAILED — kept in memory only"
              : `id ${health.supervisor.last_finalized.recording_id}`,
          ],
          ["Path", health.supervisor.last_finalized.path],
          ["Markers", String(health.supervisor.last_finalized.markers.length)],
        ]}
      />
    </Card>
  {/if}
{/if}

{#if env}
  <Card title="Environment">
    <KeyValues
      pairs={[
        ["Version", `${env.app_version} (${env.build_profile})`],
        ["Platform", `${env.os}/${env.arch} · Tauri ${env.tauri_version}`],
        ["Recorder", env.recorder_backend],
        ["Identifier", env.identifier],
        ["Database", env.db_path],
        ["Recordings", env.recordings_dir],
        ["Fixtures", env.fixtures_dir],
        ["Repo fixtures", env.repo_fixtures_dir],
        ["fixtures/sample.mp4", env.sample_mp4_present ? "present" : "not checked in"],
        ["Lockfile override", env.lockfile_override],
      ]}
    />
    <div class="row" style="margin-top:.7rem">
      <button type="button" class="tiny ghost" onclick={() => void reveal("recordings")}>
        Reveal recordings
      </button>
      <button type="button" class="tiny ghost" onclick={() => void reveal("app_data")}>
        Reveal app data
      </button>
      <button type="button" class="tiny ghost" onclick={() => void reveal("fixtures")}>
        Reveal fixtures
      </button>
    </div>
    {#if !env.sample_mp4_present}
      <p class="hint-block">
        Without <code>fixtures/sample.mp4</code> the stub recorder and the seeder both write
        placeholder files, which no demuxer will open &mdash; seeded recordings will appear in the
        library but will not play. Drop a short real clip there to test the review player. It is
        gitignored except for the <code>!fixtures/*.mp4</code> whitelist.
      </p>
    {/if}
  </Card>
{/if}
