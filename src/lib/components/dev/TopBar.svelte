<!--
  The portal's header: health pills, the Live toggle, and where writes land.
-->

<script lang="ts">
import { bytes } from "../../dev/format";
import { dev, setLive, splitDbPath } from "../../stores/dev.svelte";
import Pill from "./Pill.svelte";

const health = $derived(dev.health);
const dbPath = $derived(dev.env ? splitDbPath(dev.env.db_path) : null);

/** Under a gigabyte is the point the recorder starts refusing. */
const lowSpace = $derived((health?.free_bytes ?? Number.POSITIVE_INFINITY) < 1024 ** 3);
</script>

<header class="dev-topbar">
  <div class="dev-brand">
    <span class="dev-brand-mark">🧪</span>
    <span class="dev-brand-text">dev portal</span>
  </div>

  <div class="dev-pills">
    {#if !health}
      <Pill
        label="backend"
        value={dev.healthError ? "unreachable" : "…"}
        tone={dev.healthError ? "danger" : ""}
      />
    {:else}
      {@const state = health.supervisor.state}
      <Pill label="state" value={state} tone={state === "Recording" ? "ok" : ""} live={state === "Recording"} />
      {#if health.is_recording}<Pill label="recorder" value="capturing" tone="ok" live />{/if}
      {#if health.replay_running}<Pill label="replay" value="running" tone="warn" live />{/if}
      {#if health.fixture_recording}<Pill label="fixtures" value="capturing" tone="warn" />{/if}
      <Pill label="library" value={String(health.counts.recordings)} />
      <Pill label="used" value={bytes(health.total_bytes)} />
      <Pill label="free" value={bytes(health.free_bytes)} tone={lowSpace ? "danger" : ""} />
    {/if}
  </div>

  <div class="dev-topbar-right">
    <label class="dev-poll-toggle">
      <input
        type="checkbox"
        checked={dev.live}
        onchange={(e) => setLive((e.currentTarget as HTMLInputElement).checked)}
      />
      Live
    </label>
    {#if dbPath && dev.env}
      <!--
        Split at the last separator so only the directory half can shrink.
        See `.dev-dbpath` in the stylesheet for why this is not a CSS ellipsis.
      -->
      <code class="dev-dbpath" title="Every write on this page lands in {dev.env.db_path}">
        <span class="dir">{dbPath.dir}</span><span class="file">{dbPath.file}</span>
      </code>
    {/if}
  </div>
</header>
