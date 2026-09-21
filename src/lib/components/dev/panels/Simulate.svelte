<!--
  Driving the state machine and the marker pipeline with no League running.

  Everything on this page pushes synthetic input through the real code path;
  the input itself lives in `lib/dev/simulate.ts`.

  **The `onHealth` hook is gone.** A panel used to be handed each 1 Hz health
  tick so it could patch the state strip in place, because a full redraw would
  have blown away whatever was typed in the snapshot box. The box is component
  state now, so the strip can simply read the shared poll.
-->

<script lang="ts">
import { tryCall } from "../../../../dev/ipc";
import {
  type DevSessionView,
  type DispatchReport,
  type FixturesState,
  GAME_STATES,
  type InjectReport,
  type ReplayStatus,
} from "../../../../dev/types";
import { devContext } from "../../../dev/context";
import { duration } from "../../../dev/format";
import { DEFAULT_REPLAY_EVENTS, EVENTS, type StateEvent } from "../../../dev/simulate";
import { dev } from "../../../stores/dev.svelte";
import { devToast } from "../../../stores/devToast.svelte";
import Card from "../Card.svelte";
import KeyValues from "../KeyValues.svelte";
import Output from "../Output.svelte";
import PanelHead from "../PanelHead.svelte";

const ctx = devContext();

let machineState = $state<string>("Idle");
let session = $state<DevSessionView | null>(null);
let lastDispatch = $state<{ value: unknown; error: boolean } | null>(null);
let lastInject = $state<{ value: unknown; error: boolean } | null>(null);
let replay = $state<ReplayStatus | null>(null);
let fixtures = $state<FixturesState | null>(null);
let probeOut = $state<{ value: unknown; error: boolean } | null>(null);

// A fixture handed over from the Fixtures panel. 99 KB of nested JSON, which
// is why it does not travel in the URL.
let snapshotJson = $state(ctx.takeHandoff() ?? "");

let replayDuration = $state(1200);
let replaySpeed = $state(60);
let replayDrive = $state(true);

let lcuPath = $state("/lol-gameflow/v1/gameflow-phase");
let championId = $state<number | null>(62);
let gameId = $state<number | null>(null);
let patchRecordingId = $state<number | null>(null);
let patchIsCustom = $state(false);

const liveClientFixtures = $derived(
  (fixtures?.entries ?? []).filter(
    (f) => f.group.includes("live-client") || f.name.includes("allgamedata"),
  ),
);

// One poll, fanned out: the strip and the session summary read the shared
// `dev_health` rather than starting a timer of their own.
$effect(() => {
  const health = dev.health;
  if (!health) return;
  machineState = health.supervisor.state;
  session = health.session;
});

$effect(() => {
  void (async () => {
    const [f, r] = await Promise.all([
      tryCall<FixturesState>("dev_fixtures_state"),
      tryCall<ReplayStatus>("dev_replay_status"),
    ]);
    fixtures = f.ok ? f.value : null;
    replay = r.ok ? r.value : null;
  })();
});

// A replay ticks faster than the 1 Hz health poll, so it gets its own while
// it is running and stops the moment it is not.
$effect(() => {
  if (!replay?.running) return;
  const timer = setInterval(async () => {
    const result = await tryCall<ReplayStatus>("dev_replay_status");
    if (!result.ok) return;
    replay = result.value;
    if (!result.value.running && result.value.finished) devToast("Replay finished", "ok");
  }, 500);
  return () => clearInterval(timer);
});

async function dispatch(spec: StateEvent) {
  const result = await tryCall<DispatchReport>("dev_dispatch_state_event", { event: spec.event });
  if (!result.ok) {
    lastDispatch = { value: result.error, error: true };
    return;
  }
  const r = result.value;
  machineState = r.after.state;
  session = r.session;
  lastDispatch = {
    value:
      r.before.state === r.after.state
        ? `${r.before.state} → (no change). ${spec.note}`
        : `${r.before.state} → ${r.after.state}. ${spec.note}`,
    error: false,
  };
}

async function loadFixture(path: string) {
  if (!path) return;
  const result = await tryCall<string>("dev_fixture_read", { path });
  if (result.ok) {
    snapshotJson = result.value;
    devToast("Fixture loaded", "ok");
  } else {
    devToast(result.error, "err");
  }
}

async function probeLive() {
  const result = await tryCall<unknown>("dev_live_client_probe");
  if (result.ok) {
    snapshotJson = JSON.stringify(result.value, null, 2);
    devToast("Fetched live snapshot", "ok");
  } else {
    devToast(result.error, "err");
  }
}

async function inject() {
  let parsed: unknown;
  try {
    parsed = JSON.parse(snapshotJson);
  } catch (err) {
    lastInject = { value: `Not valid JSON: ${err}`, error: true };
    return;
  }

  const result = await tryCall<InjectReport>("dev_inject_snapshot", { snapshot: parsed });
  if (!result.ok) {
    lastInject = { value: result.error, error: true };
    return;
  }
  const r = result.value;
  session = r.session;
  lastInject = {
    value: r.accepted
      ? `Accepted in state ${r.state}: +${r.markers_added} marker(s), +${r.samples_added} sample(s).`
      : r.note,
    error: !r.accepted,
  };
}

async function startReplay() {
  let base: unknown;
  try {
    base = JSON.parse(snapshotJson);
  } catch {
    devToast("Load a base snapshot into the box above first", "err");
    return;
  }

  const result = await tryCall<ReplayStatus>("dev_replay_start", {
    spec: {
      base_snapshot: base,
      duration_s: replayDuration,
      speed: replaySpeed,
      events: DEFAULT_REPLAY_EVENTS,
      drive_state_machine: replayDrive,
    },
  });

  if (!result.ok) {
    devToast(result.error, "err");
    return;
  }
  devToast("Replay started");
  // Starts the 500 ms poll above, which needs a running status to react to.
  const status = await tryCall<ReplayStatus>("dev_replay_status");
  if (status.ok) replay = status.value;
}

async function stopReplay() {
  await tryCall("dev_replay_stop");
  replay = null;
}

async function probe(command: string, args: Record<string, unknown>) {
  const result = await tryCall<unknown>(command, args);
  probeOut = { value: result.ok ? result.value : result.error, error: !result.ok };
  return result;
}

async function championName() {
  if (championId === null) {
    devToast("Enter a champion id", "err");
    return;
  }
  const result = await tryCall<string | null>("dev_champion_name", { championId });
  // `null` is a real answer here — no such id, or the fetch failed — so it
  // has to render as something, not as an empty box.
  probeOut = {
    value: result.ok ? (result.value ?? "null — no name (see the Log panel)") : result.error,
    error: !result.ok,
  };
}

async function patchSummary() {
  if (gameId === null || patchRecordingId === null) {
    devToast("Enter both a game id and a recording id", "err");
    return;
  }
  devToast("Patching — this can take a minute");
  const result = await probe("dev_patch_match_summary", {
    recordingId: patchRecordingId,
    gameId,
    isCustom: patchIsCustom,
  });
  // `false` is a real answer, not a failure: the client had no stats, the row
  // is gone, or the schedule ran out. The logs say which.
  if (result.ok) devToast(result.value === true ? "Row patched" : "Nothing patched");
}
</script>

<PanelHead
  title="Simulate"
  description="Drive the state machine and the marker pipeline without League running."
/>

<div class="warnbar warnbar-danger">
  These are not dry runs. Dispatching an event executes the supervisor's real actions —
  <code>Recorder::start</code> and <code>Recorder::stop</code> included — and a finalize writes a
  real row and a real file.
</div>

<Card title="Current state">
  <div class="states">
    {#each GAME_STATES as s, i (s)}
      {#if i > 0}<span class="state-arrow">→</span>{/if}
      <span class="state-node" class:current={s === machineState}>{s}</span>
    {/each}
  </div>

  <div style="margin-top:.8rem">
    {#if session}
      <KeyValues
        pairs={[
          ["Markers", String(session.marker_count)],
          ["Samples", String(session.sample_count)],
          ["Elapsed", duration(session.elapsed_s)],
          [
            "Alignment",
            session.alignment_offset_s === null
              ? "not aligned — no snapshot yet"
              : `${session.alignment_offset_s.toFixed(2)}s`,
          ],
        ]}
      />
      {#if session.recent_markers.length}
        <p class="hint" style="margin-top:.5rem">
          {session.recent_markers
            .slice(0, 8)
            .map((m) => `${m.kind}@${m.game_time_s.toFixed(0)}s`)
            .join("  ·  ")}
        </p>
      {/if}
    {:else}
      <p class="hint">No session open — nothing is collecting markers right now.</p>
    {/if}
  </div>
</Card>

<Card title="State events">
  <div class="row">
    {#each EVENTS as spec (spec.label)}
      <button type="button" onclick={() => void dispatch(spec)}>{spec.label}</button>
    {/each}
  </div>
  <p class="hint-block">
    A full game is: Client opened → Phase: InProgress → Live Client up → (inject snapshots) → Phase:
    EndOfGame.
  </p>
  {#if lastDispatch}
    <Output value={lastDispatch.value} isError={lastDispatch.error} />
  {/if}
</Card>

<Card title="Inject a Live Client Data snapshot">
  <div class="row" style="margin-bottom:.5rem">
    <select
      value=""
      onchange={(e) => {
        void loadFixture(e.currentTarget.value);
        e.currentTarget.value = "";
      }}
    >
      <option value="">Load a fixture…</option>
      {#each liveClientFixtures as f (f.path)}
        <option value={f.path}>{f.source} · {f.name}</option>
      {/each}
    </select>
    <button type="button" class="ghost tiny" onclick={() => void probeLive()}>
      Fetch from a running game
    </button>
  </div>

  <textarea
    rows="10"
    spellcheck="false"
    placeholder="Paste an allgamedata payload, or load one above"
    bind:value={snapshotJson}
  ></textarea>

  <div class="row" style="margin-top:.6rem">
    <button type="button" class="primary" onclick={() => void inject()}>Inject</button>
    <span class="hint">Runs the real MarkerTracker and team_diff over this payload.</span>
  </div>

  {#if lastInject}
    <Output value={lastInject.value} isError={lastInject.error} />
  {/if}
</Card>

<Card title="Scripted replay">
  <div class="field-grid">
    <label class="field">
      <span>Game length (s)</span>
      <input type="number" bind:value={replayDuration} />
    </label>
    <label class="field">
      <span>Speed multiplier</span>
      <input type="number" bind:value={replaySpeed} />
      <span class="hint">60× runs a 20-minute game in 20 seconds</span>
    </label>
  </div>

  <label class="check" style="margin-top:.6rem">
    <input type="checkbox" bind:checked={replayDrive} />
    Drive the state machine too — start recording, then finalize into a real row and file
  </label>

  <div class="row" style="margin-top:.8rem">
    <button
      type="button"
      class="primary"
      disabled={replay?.running}
      onclick={() => void startReplay()}
    >
      Start replay
    </button>
    <button type="button" class="danger" disabled={!replay?.running} onclick={() => void stopReplay()}>
      Stop
    </button>
    {#if replay}
      <span class="hint num">
        {replay.game_time_s.toFixed(0)}s / {replay.duration_s.toFixed(0)}s · {replay.ticks} ticks ·
        {replay.events_fired} events{replay.finished ? " · finished" : ""}
      </span>
    {/if}
  </div>

  {#if replay?.error}
    <Output value={replay.error} isError />
  {/if}

  <p class="hint-block">
    Each tick rewrites game time and the event list on the base payload, then pushes it through the
    same <code>on_snapshot</code> the 1 Hz poller uses — so the marker tracker's cross-poll
    de-duplication is exercised too, not bypassed. The base payload is whatever is in the snapshot
    box above; load a fixture first.
  </p>
</Card>

<Card title="League API probes">
  <div class="row">
    <label class="field field-inline" style="flex:1 1 22rem">
      <span>LCU path</span>
      <input type="text" style="flex:1" bind:value={lcuPath} />
    </label>
    <button type="button" onclick={() => void probe("dev_lcu_get", { path: lcuPath.trim() })}>
      GET
    </button>
  </div>

  <div class="row" style="margin-top:.5rem">
    <label class="field field-inline">
      <span>Champion id</span>
      <input type="number" style="width:11rem" bind:value={championId} />
    </label>
    <button type="button" onclick={() => void championName()}>champion_name</button>
    <span class="hint">
      The real lookup, not the raw document: fetches the asset store and resolves the id the way a
      finalize does. 62 must answer <code>Wukong</code> — <code>MonkeyKing</code> means the parse is
      reading <code>alias</code>.
    </span>
  </div>

  <div class="row" style="margin-top:.5rem">
    <label class="field field-inline">
      <span>Game id</span>
      <input type="number" style="width:11rem" bind:value={gameId} />
    </label>
    <button
      type="button"
      disabled={gameId === null}
      onclick={() => void probe("dev_fetch_match_summary", { gameId })}
    >
      fetch_match_summary
    </button>
    <span class="hint">
      One shot at the end-of-game block, then match history. No retries — use the patch below for
      what a real finalize does.
    </span>
  </div>

  <div class="row" style="margin-top:.5rem">
    <label class="field field-inline">
      <span>Recording id</span>
      <input type="number" style="width:11rem" bind:value={patchRecordingId} />
    </label>
    <label class="field field-inline">
      <span>Custom game</span>
      <input type="checkbox" bind:checked={patchIsCustom} />
    </label>
    <button type="button" onclick={() => void patchSummary()}>patch_match_summary</button>
    <span class="hint">
      Runs the whole deferred patch against the game id above and writes the result to that row.
      Blocks for up to a minute — that is the real retry schedule.
    </span>
  </div>

  {#if probeOut}
    <Output value={probeOut.value} isError={probeOut.error} />
  {/if}
</Card>
