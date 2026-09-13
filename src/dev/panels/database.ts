/**
 * Table browser, schema-generated row editor, and SQL console.
 *
 * Forms are built from `PRAGMA table_info` at runtime rather than from a
 * hardcoded column list, so a new migration shows up here without a
 * frontend change — which matters given the TS types are hand-mirrored
 * and would otherwise be the thing that goes stale first.
 */
import type { Panel, PanelContext } from "../main";
import { call, tryCall } from "../ipc";
import { card, confirmDialog, escapeHtml, output, panelHead, table, toast } from "../ui";
import type { QueryResult, TableSchema } from "../types";

let root: HTMLElement | null = null;
let ctx: PanelContext | null = null;
let schemas: TableSchema[] = [];
let activeTable = "recordings";
let page: QueryResult | null = null;
let offset = 0;
let orderBy = "";
let editing: Record<string, unknown> | null = null;
let sqlResult: { value: unknown; error: boolean } | null = null;

const LIMIT = 50;
const SNIPPET_KEY = "ninja-dev-sql-snippets";
const DEFAULT_SNIPPETS: Array<[string, string]> = [
  ["Recordings with markers", "SELECT r.id, r.champion, COUNT(m.id) AS markers\nFROM recordings r LEFT JOIN markers m ON m.recording_id = r.id\nGROUP BY r.id ORDER BY markers DESC"],
  ["Marker kinds", "SELECT kind, COUNT(*) AS n FROM markers GROUP BY kind ORDER BY n DESC"],
  ["Orphaned markers", "SELECT * FROM markers WHERE recording_id NOT IN (SELECT id FROM recordings)"],
  ["Size by pinned", "SELECT pinned, COUNT(*) AS n, SUM(size_bytes) AS bytes FROM recordings GROUP BY pinned"],
];

function schema(): TableSchema | undefined {
  return schemas.find((s) => s.name === activeTable);
}

function savedSnippets(): Array<[string, string]> {
  try {
    const raw = localStorage.getItem(SNIPPET_KEY);
    return raw ? JSON.parse(raw) : [];
  } catch {
    return [];
  }
}

function editorHtml(): string {
  const s = schema();
  if (!s) return "";
  const values = editing ?? {};
  const isNew = editing === null || values.id === undefined;

  const fields = s.columns
    .map((col) => {
      const value = values[col.name];
      const text = value === null || value === undefined ? "" : String(value);
      // The primary key is shown but never editable — changing it would
      // orphan the markers and samples that reference it.
      const disabled = col.pk ? "disabled" : "";
      return `<label class="field">
        <span>${escapeHtml(col.name)} <span class="hint">${escapeHtml(col.decl_type || "ANY")}${
          col.not_null ? " · not null" : ""
        }${col.pk ? " · pk" : ""}</span></span>
        <input type="text" data-col="${escapeHtml(col.name)}" value="${escapeHtml(text)}"
          placeholder="${col.not_null ? "" : "null"}" spellcheck="false" ${disabled} />
      </label>`;
    })
    .join("");

  return card(
    isNew ? `New row in ${activeTable}` : `Editing ${activeTable} #${values.id}`,
    `<p class="hint-block" style="margin-top:0">Leave a field blank for SQL <code>NULL</code>.
      Values that parse as a number or as JSON are sent as such; everything else is sent as text.</p>
     <div class="field-grid" style="margin-top:.7rem">${fields}</div>
     <div class="row" style="margin-top:.9rem">
       <button type="button" class="primary" data-save>${isNew ? "Insert" : "Save changes"}</button>
       ${
         isNew
           ? ""
           : `<button type="button" class="danger" data-delete>Delete row</button>
              ${
                activeTable === "recordings"
                  ? `<label class="check"><input type="checkbox" data-delete-file checked /> also delete the file</label>`
                  : ""
              }`
       }
       <button type="button" class="ghost" data-cancel-edit>Close</button>
     </div>`,
  );
}

function draw() {
  if (!root) return;
  const s = schema();
  const snippets = [...DEFAULT_SNIPPETS, ...savedSnippets()];

  root.innerHTML =
    panelHead(
      "Database",
      "Browse, edit, and query the live library database. Every write here is real and immediate.",
    ) +
    card(
      "Tables",
      `<div class="row">
        ${schemas
          .map(
            (t) =>
              `<button type="button" class="${t.name === activeTable ? "primary" : ""}" data-table="${t.name}">
                ${escapeHtml(t.name)} <span class="num">(${t.row_count})</span>
              </button>`,
          )
          .join("")}
        <span class="spacer"></span>
        <button type="button" data-new-row>New row</button>
      </div>` +
        (s
          ? `<div class="row" style="margin-top:.7rem">
              <label class="field field-inline"><span>Order by</span>
                <input type="text" id="order-by" value="${escapeHtml(orderBy)}"
                  placeholder="e.g. started_at DESC" list="col-list" />
              </label>
              <datalist id="col-list">${s.columns
                .map((c) => `<option value="${escapeHtml(c.name)} DESC"></option>`)
                .join("")}</datalist>
              <button type="button" data-page="-1" ${offset === 0 ? "disabled" : ""}>← Prev</button>
              <span class="hint num">rows ${offset + 1}–${offset + (page?.rows.length ?? 0)}</span>
              <button type="button" data-page="1" ${
                (page?.rows.length ?? 0) < LIMIT ? "disabled" : ""
              }>Next →</button>
            </div>`
          : ""),
    ) +
    (editing !== null ? editorHtml() : "") +
    card(
      `${activeTable} rows`,
      page
        ? table(page.columns, page.rows, {
            clickable: true,
            numericColumns: new Set(
              (schema()?.columns ?? [])
                .filter((c) => ["INTEGER", "REAL"].includes(c.decl_type.toUpperCase()))
                .map((c) => c.name),
            ),
            emptyMessage: "This table is empty.",
          })
        : `<p class="hint">Loading…</p>`,
    ) +
    card(
      "SQL console",
      `<div class="row" style="margin-bottom:.5rem">
        <select id="snippet-picker">
          <option value="">Snippets…</option>
          ${snippets
            .map((sn, i) => `<option value="${i}">${escapeHtml(sn[0])}</option>`)
            .join("")}
        </select>
        <button type="button" class="ghost tiny" data-save-snippet>Save current as snippet</button>
      </div>
      <textarea id="sql" rows="5" spellcheck="false"
        placeholder="SELECT * FROM recordings ORDER BY started_at DESC LIMIT 20"></textarea>
      <div class="row" style="margin-top:.6rem">
        <button type="button" class="primary" data-run-sql>Run</button>
        <span class="hint">⌘/Ctrl + Enter</span>
      </div>
      ${sqlResult ? output(sqlResult.value, sqlResult.error) : ""}`,
    ) +
    card(
      "Reset",
      `<p class="hint-block" style="margin-top:0">Empties every table, resets the autoincrement
        counters, and restores the default 50 GiB / 30 day retention policy — the state the app
        is in on a first launch.</p>
      <div class="row" style="margin-top:.7rem">
        <button type="button" class="danger" data-reset>Reset database</button>
        <label class="check"><input type="checkbox" id="reset-files" /> also delete every video file</label>
      </div>`,
      "card-danger",
    );

  wire();
}

function wire() {
  if (!root) return;

  root.querySelector<HTMLInputElement>("#order-by")?.addEventListener("change", (e) => {
    orderBy = (e.target as HTMLInputElement).value.trim();
    offset = 0;
    void loadPage();
  });

  const sql = root.querySelector<HTMLTextAreaElement>("#sql");
  sql?.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      void runSql();
    }
  });

  root.querySelector<HTMLSelectElement>("#snippet-picker")?.addEventListener("change", (e) => {
    const index = Number((e.target as HTMLSelectElement).value);
    const all = [...DEFAULT_SNIPPETS, ...savedSnippets()];
    if (sql && all[index]) sql.value = all[index][1];
  });
}

async function loadSchema() {
  const result = await tryCall<TableSchema[]>("dev_schema");
  if (result.ok) schemas = result.value;
  else toast(result.error, "err");
}

async function loadPage() {
  const result = await tryCall<QueryResult>("dev_table_page", {
    table: activeTable,
    limit: LIMIT,
    offset,
    orderBy: orderBy || undefined,
  });
  if (!root) return;
  if (result.ok) {
    page = result.value;
  } else {
    page = null;
    toast(result.error, "err");
  }
  draw();
}

/** Text → the JSON value the backend binds. Empty means NULL. */
function parseCell(raw: string): unknown {
  const text = raw.trim();
  if (text === "") return null;
  if (text === "true") return true;
  if (text === "false") return false;
  if (/^-?\d+(\.\d+)?$/.test(text)) return Number(text);
  if (text.startsWith("{") || text.startsWith("[")) {
    try {
      return JSON.parse(text);
    } catch {
      return text;
    }
  }
  return text;
}

function collectRow(): { id: number | null; values: Record<string, unknown> } {
  const values: Record<string, unknown> = {};
  let id: number | null = null;
  if (!root) return { id, values };

  root.querySelectorAll<HTMLInputElement>("[data-col]").forEach((input) => {
    const name = input.dataset.col!;
    const parsed = parseCell(input.value);
    if (input.disabled) {
      // The primary key column: carried for the WHERE clause, never in SET.
      if (name === "id" && typeof parsed === "number") id = parsed;
      return;
    }
    values[name] = parsed;
  });

  return { id, values };
}

async function runSql() {
  const editor = root?.querySelector<HTMLTextAreaElement>("#sql");
  const sql = editor?.value.trim();
  if (!sql) return;

  const result = await tryCall<QueryResult>("dev_sql_query", { sql });
  // The panel may have been navigated away from while the query ran.
  if (!root) return;
  if (!result.ok) {
    sqlResult = { value: result.error, error: true };
    draw();
    return;
  }

  const q = result.value;
  sqlResult = {
    value: q.returned_rows
      ? { columns: q.columns, rows: q.rows, elapsed_ms: Number(q.elapsed_ms.toFixed(2)) }
      : `${q.rows_affected} row(s) affected in ${q.elapsed_ms.toFixed(2)} ms`,
    error: false,
  };
  await loadSchema();
  await loadPage();
  // `loadPage` redraws, so the textarea is a fresh element — put the query
  // back so it can be edited and re-run.
  const restored = root?.querySelector<HTMLTextAreaElement>("#sql");
  if (restored) restored.value = sql;
}

export const databasePanel: Panel = {
  id: "database",
  title: "Database",
  icon: "▦",
  group: "Data",

  async mount(el, context) {
    root = el;
    ctx = context;
    editing = null;
    sqlResult = null;
    await loadSchema();
    await loadPage();

    el.addEventListener("click", async (e) => {
      const target = e.target as HTMLElement;

      const tableBtn = target.closest<HTMLElement>("[data-table]");
      if (tableBtn) {
        activeTable = tableBtn.dataset.table!;
        offset = 0;
        orderBy = "";
        editing = null;
        await loadPage();
        return;
      }

      const pageBtn = target.closest<HTMLElement>("[data-page]");
      if (pageBtn) {
        offset = Math.max(0, offset + Number(pageBtn.dataset.page) * LIMIT);
        await loadPage();
        return;
      }

      if (target.closest("[data-new-row]")) {
        editing = {};
        draw();
        return;
      }

      if (target.closest("[data-cancel-edit]")) {
        editing = null;
        draw();
        return;
      }

      const row = target.closest<HTMLElement>("tr[data-row-index]");
      if (row && page) {
        const index = Number(row.dataset.rowIndex);
        editing = Object.fromEntries(page.columns.map((c, i) => [c, page!.rows[index][i]]));
        draw();
        root!.querySelector(".card")?.scrollIntoView({ block: "nearest" });
        return;
      }

      if (target.closest("[data-save]")) {
        const { id, values } = collectRow();
        try {
          if (id === null) {
            const newId = await call<number>("dev_insert_row", { table: activeTable, values });
            toast(`Inserted row ${newId}`, "ok");
          } else {
            const changed = await call<number>("dev_update_row", { table: activeTable, id, values });
            toast(`Updated ${changed} row(s)`, "ok");
          }
          editing = null;
          await loadSchema();
          await loadPage();
        } catch (err) {
          toast(String(err), "err");
        }
        return;
      }

      if (target.closest("[data-delete]")) {
        const { id } = collectRow();
        if (id === null) return;
        const deleteFile =
          root?.querySelector<HTMLInputElement>("[data-delete-file]")?.checked ?? false;
        const ok = await confirmDialog({
          title: `Delete ${activeTable} #${id}?`,
          body: deleteFile
            ? "The row and its video file will both be removed. This cannot be undone."
            : "The row will be removed. Its file stays on disk, so the next rescan will import it back as an untracked recording.",
          confirmLabel: "Delete",
        });
        if (!ok) return;
        try {
          await call("dev_delete_row", { table: activeTable, id, deleteFile });
          toast("Deleted", "ok");
          editing = null;
          await loadSchema();
          await loadPage();
        } catch (err) {
          toast(String(err), "err");
        }
        return;
      }

      if (target.closest("[data-run-sql]")) {
        await runSql();
        return;
      }

      if (target.closest("[data-save-snippet]")) {
        const sql = root?.querySelector<HTMLTextAreaElement>("#sql")?.value.trim();
        if (!sql) return;
        const name = prompt("Name this snippet:");
        if (!name) return;
        localStorage.setItem(SNIPPET_KEY, JSON.stringify([...savedSnippets(), [name, sql]]));
        toast("Snippet saved", "ok");
        draw();
        return;
      }

      if (target.closest("[data-reset]")) {
        const alsoClearFiles =
          root?.querySelector<HTMLInputElement>("#reset-files")?.checked ?? false;
        const ok = await confirmDialog({
          title: "Reset the database?",
          body: `Every recording, marker, and sample row will be deleted${
            alsoClearFiles ? ", along with every .mp4 and .mkv in the recordings folder" : ""
          }. The retention policy returns to its 50 GiB / 30 day default.`,
          confirmLabel: "Reset everything",
          typeToConfirm: "reset",
        });
        if (!ok) return;
        try {
          const report = await call<{ rows_deleted: number; files_deleted: number }>("dev_reset_db", {
            alsoClearFiles,
          });
          toast(
            `Reset: ${report.rows_deleted} row(s), ${report.files_deleted} file(s) removed`,
            "ok",
          );
          ctx?.refresh();
        } catch (err) {
          toast(String(err), "err");
        }
      }
    });
  },

  unmount() {
    root = null;
    ctx = null;
  },
};
