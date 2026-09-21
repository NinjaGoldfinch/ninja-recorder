<!--
  Table browser, schema-generated row editor, and SQL console.

  Forms are built from `PRAGMA table_info` at runtime rather than from a
  hardcoded column list, so a new migration shows up here without a frontend
  change — which matters given the TS types are hand-mirrored and would
  otherwise be the thing that goes stale first.
-->

<script lang="ts">
import { call, tryCall } from "../../../../dev/ipc";
import type { QueryResult, TableSchema } from "../../../../dev/types";
import { devContext } from "../../../dev/context";
import { allSnippets, parseCell, saveSnippet } from "../../../dev/sql";
import { devToast } from "../../../stores/devToast.svelte";
import Card from "../Card.svelte";
import DataTable from "../DataTable.svelte";
import Output from "../Output.svelte";
import PanelHead from "../PanelHead.svelte";

const LIMIT = 50;

const ctx = devContext();

let schemas = $state<TableSchema[]>([]);
let activeTable = $state("recordings");
let page = $state<QueryResult | null>(null);
let offset = $state(0);
let orderBy = $state("");
let sqlResult = $state<{ value: unknown; error: boolean } | null>(null);
let sqlText = $state("");
let snippets = $state(allSnippets());
let resetFiles = $state(false);
let deleteFile = $state(true);

/** Bumped after a write, to re-read the page the write changed. */
let nonce = $state(0);

/**
 * The open row editor: every column as text, plus the primary key it was
 * loaded from. `null` when nothing is open, and a `null` id means a new row.
 */
let editing = $state<{ id: number | null; values: Record<string, string> } | null>(null);

const schema = $derived(schemas.find((s) => s.name === activeTable));

const numericColumns = $derived(
  new Set(
    (schema?.columns ?? [])
      .filter((c) => ["INTEGER", "REAL"].includes(c.decl_type.toUpperCase()))
      .map((c) => c.name),
  ),
);

$effect(() => {
  void nonce;
  void loadSchema();
});

$effect(() => {
  void loadPage(activeTable, offset, orderBy, nonce);
});

async function loadSchema() {
  const result = await tryCall<TableSchema[]>("dev_schema");
  if (result.ok) schemas = result.value;
  else devToast(result.error, "err");
}

async function loadPage(table: string, from: number, order: string, _nonce: number) {
  const result = await tryCall<QueryResult>("dev_table_page", {
    table,
    limit: LIMIT,
    offset: from,
    orderBy: order || undefined,
  });
  if (result.ok) {
    page = result.value;
  } else {
    page = null;
    devToast(result.error, "err");
  }
}

/** Re-reads both the schema (row counts change) and the current page. */
function reload() {
  nonce += 1;
}

function chooseTable(name: string) {
  activeTable = name;
  offset = 0;
  orderBy = "";
  editing = null;
}

function asText(value: unknown): string {
  return value === null || value === undefined ? "" : String(value);
}

function editRow(index: number) {
  if (!page) return;
  const values: Record<string, string> = {};
  page.columns.forEach((column, i) => {
    values[column] = asText(page?.rows[index][i]);
  });
  const id = Number(values.id);
  editing = { id: Number.isFinite(id) ? id : null, values };
}

function newRow() {
  const values: Record<string, string> = {};
  for (const column of schema?.columns ?? []) values[column.name] = "";
  editing = { id: null, values };
}

/** The editor's fields as the values the backend binds. The primary key is
 *  carried for the WHERE clause and never appears in SET — changing it would
 *  orphan the markers and samples that reference it. */
function collect(): Record<string, unknown> {
  const values: Record<string, unknown> = {};
  for (const column of schema?.columns ?? []) {
    if (column.pk) continue;
    values[column.name] = parseCell(editing?.values[column.name] ?? "");
  }
  return values;
}

async function save() {
  if (!editing) return;
  try {
    if (editing.id === null) {
      const id = await call<number>("dev_insert_row", { table: activeTable, values: collect() });
      devToast(`Inserted row ${id}`, "ok");
    } else {
      const changed = await call<number>("dev_update_row", {
        table: activeTable,
        id: editing.id,
        values: collect(),
      });
      devToast(`Updated ${changed} row(s)`, "ok");
    }
    editing = null;
    reload();
  } catch (err) {
    devToast(String(err), "err");
  }
}

async function remove() {
  const id = editing?.id;
  if (id === null || id === undefined) return;

  const ok = await ctx.confirm({
    title: `Delete ${activeTable} #${id}?`,
    body:
      activeTable === "recordings" && deleteFile
        ? "The row and its video file will both be removed. This cannot be undone."
        : "The row will be removed. Its file stays on disk, so the next rescan will import it back as an untracked recording.",
    confirmLabel: "Delete",
  });
  if (!ok) return;

  try {
    // The checkbox is only offered for `recordings`; no other table has a
    // file to delete, and the backend should not be asked to find one.
    await call("dev_delete_row", {
      table: activeTable,
      id,
      deleteFile: activeTable === "recordings" && deleteFile,
    });
    devToast("Deleted", "ok");
    editing = null;
    reload();
  } catch (err) {
    devToast(String(err), "err");
  }
}

async function runSql() {
  const sql = sqlText.trim();
  if (!sql) return;

  const result = await tryCall<QueryResult>("dev_sql_query", { sql });
  if (!result.ok) {
    sqlResult = { value: result.error, error: true };
    return;
  }

  const q = result.value;
  sqlResult = {
    value: q.returned_rows
      ? { columns: q.columns, rows: q.rows, elapsed_ms: Number(q.elapsed_ms.toFixed(2)) }
      : `${q.rows_affected} row(s) affected in ${q.elapsed_ms.toFixed(2)} ms`,
    error: false,
  };
  // A write through the console changes what the browser above is showing.
  // The query itself stays in the textarea, which is component state now
  // rather than an element the redraw used to replace.
  reload();
}

function storeSnippet() {
  const sql = sqlText.trim();
  if (!sql) return;
  const name = prompt("Name this snippet:");
  if (!name) return;
  saveSnippet(name, sql);
  snippets = allSnippets();
  devToast("Snippet saved", "ok");
}

async function reset() {
  const ok = await ctx.confirm({
    title: "Reset the database?",
    body: `Every recording, marker, and sample row will be deleted${
      resetFiles ? ", along with every .mp4 and .mkv in the recordings folder" : ""
    }. The retention policy returns to its 50 GiB / 30 day default.`,
    confirmLabel: "Reset everything",
    typeToConfirm: "reset",
  });
  if (!ok) return;

  try {
    const report = await call<{ rows_deleted: number; files_deleted: number }>("dev_reset_db", {
      alsoClearFiles: resetFiles,
    });
    devToast(`Reset: ${report.rows_deleted} row(s), ${report.files_deleted} file(s) removed`, "ok");
    editing = null;
    offset = 0;
    reload();
  } catch (err) {
    devToast(String(err), "err");
  }
}
</script>

<PanelHead
  title="Database"
  description="Browse, edit, and query the live library database. Every write here is real and immediate."
/>

<Card title="Tables">
  <div class="row">
    {#each schemas as t (t.name)}
      <button type="button" class:primary={t.name === activeTable} onclick={() => chooseTable(t.name)}>
        {t.name} <span class="num">({t.row_count})</span>
      </button>
    {/each}
    <span class="spacer"></span>
    <button type="button" onclick={newRow} disabled={!schema}>New row</button>
  </div>

  {#if schema}
    <div class="row" style="margin-top:.7rem">
      <label class="field field-inline">
        <span>Order by</span>
        <!-- `onchange`, not `oninput`: this is a query per keystroke otherwise. -->
        <input
          type="text"
          value={orderBy}
          placeholder="e.g. started_at DESC"
          list="col-list"
          onchange={(e) => {
            orderBy = e.currentTarget.value.trim();
            offset = 0;
          }}
        />
      </label>
      <datalist id="col-list">
        {#each schema.columns as column (column.name)}
          <option value="{column.name} DESC"></option>
        {/each}
      </datalist>
      <button
        type="button"
        disabled={offset === 0}
        onclick={() => (offset = Math.max(0, offset - LIMIT))}
      >
        ← Prev
      </button>
      <span class="hint num">rows {offset + 1}–{offset + (page?.rows.length ?? 0)}</span>
      <button
        type="button"
        disabled={(page?.rows.length ?? 0) < LIMIT}
        onclick={() => (offset += LIMIT)}
      >
        Next →
      </button>
    </div>
  {/if}
</Card>

{#if editing && schema}
  <Card title={editing.id === null ? `New row in ${activeTable}` : `Editing ${activeTable} #${editing.id}`}>
    <p class="hint-block" style="margin-top:0">
      Leave a field blank for SQL <code>NULL</code>. Values that parse as a number or as JSON are
      sent as such; everything else is sent as text.
    </p>

    <div class="field-grid" style="margin-top:.7rem">
      {#each schema.columns as column (column.name)}
        <label class="field">
          <span>
            {column.name}
            <span class="hint">
              {column.decl_type || "ANY"}{column.not_null ? " · not null" : ""}{column.pk
                ? " · pk"
                : ""}
            </span>
          </span>
          <input
            type="text"
            spellcheck="false"
            placeholder={column.not_null ? "" : "null"}
            disabled={column.pk}
            bind:value={editing.values[column.name]}
          />
        </label>
      {/each}
    </div>

    <div class="row" style="margin-top:.9rem">
      <button type="button" class="primary" onclick={() => void save()}>
        {editing.id === null ? "Insert" : "Save changes"}
      </button>
      {#if editing.id !== null}
        <button type="button" class="danger" onclick={() => void remove()}>Delete row</button>
        {#if activeTable === "recordings"}
          <label class="check">
            <input type="checkbox" bind:checked={deleteFile} /> also delete the file
          </label>
        {/if}
      {/if}
      <button type="button" class="ghost" onclick={() => (editing = null)}>Close</button>
    </div>
  </Card>
{/if}

<Card title="{activeTable} rows">
  {#if page}
    <DataTable
      columns={page.columns}
      rows={page.rows}
      {numericColumns}
      onrow={editRow}
      emptyMessage="This table is empty."
    />
  {:else}
    <p class="hint">Loading…</p>
  {/if}
</Card>

<Card title="SQL console">
  <div class="row" style="margin-bottom:.5rem">
    <select
      value=""
      onchange={(e) => {
        const chosen = snippets[Number(e.currentTarget.value)];
        if (chosen) sqlText = chosen[1];
        e.currentTarget.value = "";
      }}
    >
      <option value="">Snippets…</option>
      {#each snippets as snippet, i (`${i}:${snippet[0]}`)}
        <option value={i}>{snippet[0]}</option>
      {/each}
    </select>
    <button type="button" class="ghost tiny" onclick={storeSnippet}>Save current as snippet</button>
  </div>

  <textarea
    rows="5"
    spellcheck="false"
    placeholder="SELECT * FROM recordings ORDER BY started_at DESC LIMIT 20"
    bind:value={sqlText}
    onkeydown={(e) => {
      if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
        e.preventDefault();
        void runSql();
      }
    }}
  ></textarea>

  <div class="row" style="margin-top:.6rem">
    <button type="button" class="primary" onclick={() => void runSql()}>Run</button>
    <span class="hint">⌘/Ctrl + Enter</span>
  </div>

  {#if sqlResult}
    <Output value={sqlResult.value} isError={sqlResult.error} />
  {/if}
</Card>

<Card title="Reset" extraClass="card-danger">
  <p class="hint-block" style="margin-top:0">
    Empties every table, resets the autoincrement counters, and restores the default 50 GiB / 30 day
    retention policy — the state the app is in on a first launch.
  </p>
  <div class="row" style="margin-top:.7rem">
    <button type="button" class="danger" onclick={() => void reset()}>Reset database</button>
    <label class="check">
      <input type="checkbox" bind:checked={resetFiles} /> also delete every video file
    </label>
  </div>
</Card>
