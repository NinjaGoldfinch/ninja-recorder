<!--
  Two logs, one panel.

  **Backend** is the file `log.rs` writes — the one a release build has too,
  which is the whole point: a shipped app has no console, so until that file
  existed every diagnostic went to a closed handle (DEVELOPMENT.md §13).

  **Portal IPC** is every call this page has made.

  The filters are component state, so `r` resets them. They used to be module
  state because `refresh()` rebuilt the panel from scratch and would otherwise
  have lost them; `r` is now the only thing that remounts this panel, and
  "reload everything" is what it says.
-->

<script lang="ts">
import { clearLog, type LogEntry, logEntries, onLog, tryCall } from "../../../../dev/ipc";
import type { LogFileInfo, LogPage } from "../../../../dev/types";
import { bytes, clockTime } from "../../../dev/format";
import {
  argSummary,
  HIDDEN_TAGS,
  LEVELS,
  levelTone,
  toggleLevel,
  toggleTag,
  visibleEntries,
} from "../../../dev/log";
import { devToast } from "../../../stores/devToast.svelte";
import Output from "../Output.svelte";
import PanelHead from "../PanelHead.svelte";

let source = $state<"backend" | "ipc">("backend");

// --- Backend -------------------------------------------------------

let files = $state<LogFileInfo[]>([]);
let page = $state<LogPage | null>(null);
let backendFile = $state<string | null>(null);
let backendSearch = $state("");
let levels = $state<string[]>([...LEVELS]);
let hiddenTags = $state<string[]>([...HIDDEN_TAGS]);

/**
 * Which query the newest render belongs to.
 *
 * Two filter clicks in quick succession are two reads of a file that can be
 * five megabytes, and nothing makes them come back in order — the slower first
 * one would otherwise repaint the list with results for a filter that is no
 * longer set.
 */
let request = 0;

/** Bumped by the Reload button. The effect below re-runs on any change to the
 *  query, and "the same query again" is not one. */
let nonce = $state(0);

$effect(() => {
  if (source !== "backend") return;
  void nonce;
  // Read every dependency before the first await, or the effect tracks none
  // of them.
  void loadBackend({
    file: backendFile,
    levels: [...levels],
    hideTags: [...hiddenTags],
    search: backendSearch,
    limit: 500,
  });
});

async function loadBackend(query: Record<string, unknown>) {
  const mine = ++request;
  const [filesResult, pageResult] = await Promise.all([
    tryCall<LogFileInfo[]>("dev_log_files"),
    tryCall<LogPage>("dev_read_log", { query }),
  ]);
  if (mine !== request) return;

  if (filesResult.ok) files = filesResult.value;
  if (pageResult.ok) {
    page = pageResult.value;
    backendFile ??= pageResult.value.file;
  } else {
    page = null;
    devToast(pageResult.error, "err");
  }
}

const summary = $derived(
  page
    ? `${page.matched} of ${page.total} lines${
        page.truncated ? `, showing the newest ${page.lines.length}` : ""
      }`
    : "no log file read",
);

function fileLabel(f: LogFileInfo): string {
  const size = f.exists ? bytes(f.bytes) : "not created yet";
  return `${f.name}${f.active ? " (active)" : ""} — ${size}`;
}

// --- Portal IPC ----------------------------------------------------

let entries = $state<readonly LogEntry[]>(logEntries());
let ipcSearch = $state("");
let showPolled = $state(false);
let expanded = $state<number | null>(null);

$effect(() => onLog((next) => (entries = next)));

const visible = $derived(visibleEntries(entries, { search: ipcSearch, showPolled }));

async function copyIpc() {
  try {
    await navigator.clipboard.writeText(JSON.stringify(visible, null, 2));
    devToast("Copied to clipboard", "ok");
  } catch (err) {
    devToast(`Clipboard unavailable: ${err}`, "err");
  }
}
</script>

<PanelHead
  title="Log"
  description="The backend's own log file — the one a release build writes too — and every IPC call this page has made."
/>

<div class="row" style="margin-bottom:.8rem">
  <button
    type="button"
    class="chip"
    class:on={source === "backend"}
    onclick={() => (source = "backend")}
  >
    Backend
  </button>
  <button type="button" class="chip" class:on={source === "ipc"} onclick={() => (source = "ipc")}>
    Portal IPC
  </button>
</div>

{#if source === "backend"}
  <div class="row" style="margin-bottom:.5rem">
    <label class="field field-inline" style="flex:1 1 20rem">
      <span>File</span>
      <select style="flex:1" bind:value={backendFile}>
        {#each files as f (f.name)}
          <option value={f.name} disabled={!f.exists}>{fileLabel(f)}</option>
        {/each}
      </select>
    </label>
    <button type="button" class="ghost" onclick={() => (nonce += 1)}>Reload</button>
    <button
      type="button"
      class="ghost"
      onclick={() => void tryCall("dev_open_data_dir", { which: "app_data" })}
    >
      Reveal folder
    </button>
  </div>

  <div class="row" style="margin-bottom:.5rem">
    <!-- `onchange`, not `oninput`: each keystroke is a read of a file that can
         be five megabytes. -->
    <input
      type="search"
      placeholder="Search the whole line…"
      style="flex:1 1 16rem"
      value={backendSearch}
      onchange={(e) => (backendSearch = e.currentTarget.value)}
    />
    {#each LEVELS as level (level)}
      <button
        type="button"
        class="chip"
        class:on={levels.includes(level)}
        onclick={() => (levels = toggleLevel(levels, level))}
      >
        {level}
      </button>
    {/each}
  </div>

  {#if page?.tags_present.length}
    <div class="row" style="margin-bottom:.5rem">
      <span class="hint">Tags</span>
      {#each page.tags_present as tag (tag)}
        <button
          type="button"
          class="chip"
          class:on={!hiddenTags.includes(tag)}
          onclick={() => (hiddenTags = toggleTag(hiddenTags, tag))}
        >
          {tag}
        </button>
      {/each}
    </div>
  {/if}

  <p class="hint-block" style="margin-top:0">
    {summary}. Written to <code>{page?.dir ?? "—"}</code>. <code>live-poll</code> and
    <code>libobs</code>
    are hidden by default — at 1 Hz they bury everything else. They are only written at all with
    <code>NINJA_RECORDER_LOG_LEVEL=debug</code>.
  </p>

  <div class="log-list">
    {#each page?.lines ?? [] as line, i (i)}
      <div class="log-entry {levelTone(line.level)}">
        <span class="t">{line.timestamp.slice(11, 23)}</span>
        <span class="st">{line.level || "—"}</span>
        <span class="name">{line.tag || "—"}</span>
        <span class="args">{line.message}</span>
      </div>
    {:else}
      <div class="dev-empty">No lines match.</div>
    {/each}
  </div>
{:else}
  <div class="row" style="margin-bottom:.8rem">
    <input
      type="search"
      placeholder="Filter by command, args, or error…"
      style="flex:1 1 18rem"
      bind:value={ipcSearch}
    />
    <label class="check">
      <input type="checkbox" bind:checked={showPolled} /> Show 1 Hz polls
    </label>
    <button type="button" class="ghost" onclick={() => void copyIpc()}>Copy as JSON</button>
    <button
      type="button"
      class="ghost"
      onclick={() => {
        clearLog();
        devToast("Log cleared");
      }}
    >
      Clear
    </button>
  </div>

  <div class="log-list">
    {#each visible as e (e.id)}
      <!-- svelte-ignore a11y_click_events_have_key_events -->
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div
        class="log-entry {e.ok ? 'ok' : 'err'}"
        onclick={() => (expanded = expanded === e.id ? null : e.id)}
      >
        <span class="t">{clockTime(e.at)}</span>
        <span class="name">{e.command}<span class="args"> {argSummary(e.args)}</span></span>
        <span class="ms">{e.ms.toFixed(0)} ms</span>
        <span class="st">{e.ok ? "ok" : "error"}</span>
      </div>
      {#if expanded === e.id}
        <Output
          value={e.ok ? { args: e.args, result: e.result } : { args: e.args, error: e.error }}
          isError={!e.ok}
        />
      {/if}
    {:else}
      <div class="dev-empty">No calls match.</div>
    {/each}
  </div>
{/if}
