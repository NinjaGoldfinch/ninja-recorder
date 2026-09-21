<!--
  IPC console: every command, an argument form generated from the catalogue,
  and the raw response.

  This is the answer to "test every backend feature": anything reachable over
  `invoke` is reachable here, including the commands no other panel bothers to
  surface.

  The catalogue is generated from the Rust declaration (WS2.7). It used to be
  `registry.ts`, a hand-written second copy of the command surface, and this
  panel carried a banner comparing the two at runtime. There is one list now,
  so there is nothing left to compare and the banner is gone.
-->

<script lang="ts">
import { COMMANDS, type PortalCommand } from "../../../../dev/commands.generated";
import { call } from "../../../../dev/ipc";
import { collectArgs, defaultFor, type RawArgs } from "../../../dev/args";
import { devToast } from "../../../stores/devToast.svelte";
import Card from "../Card.svelte";
import Output from "../Output.svelte";
import PanelHead from "../PanelHead.svelte";

let search = $state("");
let selectedName = $state(COMMANDS[0].name);
let result = $state<{ value: unknown; ms: number; error: boolean } | null>(null);

/**
 * Last argument values per command, so re-running one is one click.
 *
 * Keyed by command name rather than held per-field, because the form is
 * rebuilt whenever you move between commands and the point is that moving
 * away and back does not lose what you typed.
 */
const remembered = $state<Record<string, RawArgs>>({});

const selected = $derived(COMMANDS.find((c) => c.name === selectedName) ?? COMMANDS[0]);

const matching = $derived.by(() => {
  const q = search.trim().toLowerCase();
  if (!q) return COMMANDS;
  return COMMANDS.filter(
    (c) =>
      c.name.includes(q) ||
      c.group.toLowerCase().includes(q) ||
      c.description.toLowerCase().includes(q),
  );
});

/** Fills in a command's fields the first time it is opened. Called from the
 *  two places that change the selection, not from an effect: the template
 *  binds straight into this object and must not render before it exists. */
function ensureFields(command: PortalCommand): RawArgs {
  const existing = remembered[command.name];
  if (existing) return existing;
  const fresh: RawArgs = {};
  for (const spec of command.args) fresh[spec.name] = defaultFor(spec);
  remembered[command.name] = fresh;
  return fresh;
}

// The initial selection, by name rather than through `selected`: this runs
// once at setup, and reading a rune here would only capture its first value.
ensureFields(COMMANDS[0]);

function select(command: PortalCommand) {
  selectedName = command.name;
  ensureFields(command);
  result = null;
}

async function invoke() {
  const collected = collectArgs(selected.args, remembered[selected.name] ?? {});
  if (!collected.ok) {
    devToast(collected.error, "err");
    return;
  }

  const { args } = collected;
  const started = performance.now();
  try {
    const value = await call<unknown>(selected.name, Object.keys(args).length ? args : undefined);
    result = {
      value: value ?? "(no value returned)",
      ms: performance.now() - started,
      error: false,
    };
  } catch (err) {
    result = { value: String(err), ms: performance.now() - started, error: true };
  }
}

async function copyResponse() {
  if (!result) return;
  const text =
    typeof result.value === "string" ? result.value : JSON.stringify(result.value, null, 2);
  try {
    await navigator.clipboard.writeText(text);
    devToast("Copied to clipboard", "ok");
  } catch (err) {
    devToast(`Clipboard unavailable: ${err}`, "err");
  }
}
</script>

<PanelHead
  title="Commands"
  description="Every command the backend exposes, with a generated argument form. Blue dots are dev-only commands; red dots change state."
/>

<div class="split">
  <div>
    <input
      type="search"
      placeholder="Search commands…"
      style="width:100%;margin-bottom:.5rem"
      bind:value={search}
    />
    <div class="cmd-list">
      {#each matching as command, i (command.name)}
        {#if command.group !== matching[i - 1]?.group}
          <div class="dev-nav-group">{command.group}</div>
        {/if}
        <button
          type="button"
          class="cmd-item"
          class:active={command.name === selectedName}
          class:is-danger={command.danger}
          class:is-dev={!command.danger && command.dev}
          onclick={() => select(command)}
        >
          <span class="dot"></span>{command.name}
        </button>
      {/each}
      {#if matching.length === 0}
        <p class="hint">No command matches that.</p>
      {/if}
    </div>
  </div>

  <div>
    <Card title={selected.name} raw>
      <p class="hint-block" style="margin-top:0">{selected.description}</p>

      {#if selected.danger}
        <div class="warnbar warnbar-danger">
          This command writes, deletes, or otherwise changes state. It is not safely repeatable.
        </div>
      {/if}

      {#if selected.args.length}
        <div class="field-grid" style="margin-top:.8rem">
          <!--
            The help text lives *inside* the label. As a sibling it would land
            in its own `.field-grid` cell, ending up beside an unrelated field
            rather than under its own.
          -->
          {#each selected.args as spec (spec.name)}
            {@const label = `${spec.name}${spec.optional ? " (optional)" : ""}`}
            {#if spec.kind === "boolean"}
              <div class="field">
                <label class="check">
                  <input
                    type="checkbox"
                    checked={remembered[selected.name][spec.name] === "true"}
                    onchange={(e) => {
                      remembered[selected.name][spec.name] = String(e.currentTarget.checked);
                    }}
                  />
                  {label}
                </label>
                {#if spec.help}<span class="hint">{spec.help}</span>{/if}
              </div>
            {:else if spec.kind === "json"}
              <label class="field">
                <span>{label} — JSON</span>
                <textarea
                  rows="5"
                  spellcheck="false"
                  bind:value={remembered[selected.name][spec.name]}
                ></textarea>
                {#if spec.help}<span class="hint">{spec.help}</span>{/if}
              </label>
            {:else}
              <!--
                `value` + `oninput` rather than `bind:value`: the binding
                forbids a dynamic `type`, and on `type="number"` it coerces to
                a number, while every field here is held as a string so that
                one map covers all four kinds.
              -->
              <label class="field">
                <span>{label}</span>
                <input
                  type={spec.kind === "number" ? "number" : "text"}
                  spellcheck="false"
                  value={remembered[selected.name][spec.name]}
                  oninput={(e) => {
                    remembered[selected.name][spec.name] = e.currentTarget.value;
                  }}
                />
                {#if spec.help}<span class="hint">{spec.help}</span>{/if}
              </label>
            {/if}
          {/each}
        </div>
      {:else}
        <p class="hint">No arguments.</p>
      {/if}

      <div class="row" style="margin-top:.9rem">
        <button type="button" class="primary" onclick={() => void invoke()}>Invoke</button>
        {#if result}
          <button type="button" class="ghost" onclick={() => void copyResponse()}>
            Copy response
          </button>
          <span class="hint num">{result.ms.toFixed(1)} ms</span>
        {/if}
      </div>

      {#if result}
        <Output value={result.value} isError={result.error} />
      {/if}
    </Card>
  </div>
</div>
