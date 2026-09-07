/**
 * Two logs, one panel.
 *
 * **Backend** is the file `log.rs` writes — the one a release build has
 * too, which is the whole point: a shipped app has no console, so until
 * that file existed every diagnostic went to a closed handle
 * (DEVELOPMENT.md §13).
 *
 * **Portal IPC** is every call this page has made. Polled commands are
 * hidden by default — at 1 Hz they bury everything a person actually
 * clicked within seconds.
 *
 * The backend view filters server-side: the file is capped at 5 MiB, which
 * is far too much to hand a webview in one string, so `dev_read_log` does
 * the matching and returns a window.
 */
import type { Panel } from "../main";
import { clearLog, logEntries, onLog, POLLED_COMMANDS, type LogEntry } from "../ipc";
import { tryCall } from "../ipc";
import type { LogFileInfo, LogPage } from "../types";
import { bytes, clockTime, escapeHtml, output, panelHead, toast } from "../ui";

let root: HTMLElement | null = null;
let unsubscribe: (() => void) | null = null;
let showPolled = false;
let filter = "";
let expanded: number | null = null;

type Source = "backend" | "ipc";
let source: Source = "backend";

/** Every level, so an untouched panel shows the whole file. */
const LEVELS = ["ERROR", "WARN", "INFO", "DEBUG"] as const;
let levels = new Set<string>(LEVELS);
/** Tags to *hide*. The high-volume streams are off by default: at 1 Hz
 *  `live-poll` alone would bury a session's real errors. */
let hiddenTags = new Set<string>(["live-poll", "libobs"]);
let backendFile: string | null = null;
let backendSearch = "";
let page: LogPage | null = null;
let files: LogFileInfo[] = [];

/** Which query the newest render belongs to. Two filter clicks in quick
 *  succession are two reads of a file that can be five megabytes, and
 *  nothing makes them come back in order — the slower first one would
 *  otherwise repaint the list with results for a filter that is no longer
 *  set. The chips are drawn from module state either way, so only the
 *  lines could ever disagree with them. */
let backendRequest = 0;

async function loadBackend() {
  const mine = ++backendRequest;
  const [filesResult, pageResult] = await Promise.all([
    tryCall<LogFileInfo[]>("dev_log_files"),
    tryCall<LogPage>("dev_read_log", {
      query: {
        file: backendFile,
        levels: [...levels],
        // Names what to *hide*. That is what lets the noisy streams be
        // hidden on the very first render, before this panel has read the
        // file and learned which tags exist.
        hideTags: [...hiddenTags],
        search: backendSearch,
        limit: 500,
      },
    }),
  ]);
  if (mine !== backendRequest) return;

  if (filesResult.ok) files = filesResult.value;
  if (pageResult.ok) {
    page = pageResult.value;
    backendFile ??= page.file;
  } else {
    page = null;
    toast(pageResult.error, "err");
  }
  drawBackend();
}

function levelTone(level: string): string {
  if (level === "ERROR") return "err";
  if (level === "WARN") return "warn";
  return "";
}

function drawBackend() {
  const host = root?.querySelector<HTMLElement>("#backend-body");
  if (!host) return;

  const tagButtons = (page?.tags_present ?? [])
    .map((tag) => {
      const on = !hiddenTags.has(tag);
      return `<button type="button" class="chip ${on ? "on" : ""}" data-tag="${escapeHtml(tag)}">${escapeHtml(tag)}</button>`;
    })
    .join("");

  const fileOptions = files
    .map(
      (f) =>
        `<option value="${escapeHtml(f.name)}"${f.name === page?.file ? " selected" : ""}${
          f.exists ? "" : " disabled"
        }>${escapeHtml(f.name)}${f.active ? " (active)" : ""} — ${
          f.exists ? bytes(f.bytes) : "not created yet"
        }</option>`,
    )
    .join("");

  const rows = (page?.lines ?? [])
    .map(
      (l) =>
        `<div class="log-entry ${levelTone(l.level)}">
          <span class="t">${escapeHtml(l.timestamp.slice(11, 23))}</span>
          <span class="st">${escapeHtml(l.level || "—")}</span>
          <span class="name">${escapeHtml(l.tag || "—")}</span>
          <span class="args">${escapeHtml(l.message)}</span>
        </div>`,
    )
    .join("");

  const summary = page
    ? `${page.matched} of ${page.total} lines${page.truncated ? `, showing the newest ${page.lines.length}` : ""}`
    : "no log file read";

  host.innerHTML = `
    <div class="row" style="margin-bottom:.5rem">
      <label class="field field-inline" style="flex:1 1 20rem"><span>File</span>
        <select id="log-file" style="flex:1">${fileOptions}</select></label>
      <button type="button" class="ghost" data-reload>Reload</button>
      <button type="button" class="ghost" data-reveal>Reveal folder</button>
    </div>
    <div class="row" style="margin-bottom:.5rem">
      <input type="search" id="backend-search" placeholder="Search the whole line…" style="flex:1 1 16rem" />
      ${LEVELS.map(
        (l) =>
          `<button type="button" class="chip ${levels.has(l) ? "on" : ""}" data-level="${l}">${l}</button>`,
      ).join("")}
    </div>
    ${tagButtons ? `<div class="row" style="margin-bottom:.5rem"><span class="hint">Tags</span>${tagButtons}</div>` : ""}
    <p class="hint-block" style="margin-top:0">${escapeHtml(summary)}. Written to
     <code>${escapeHtml(page?.dir ?? "—")}</code>.
     <code>live-poll</code> and <code>libobs</code> are hidden by default — at 1 Hz they bury
     everything else. They are only written at all with
     <code>NINJA_RECORDER_LOG_LEVEL=debug</code>.</p>
    <div class="log-list">${rows || `<div class="dev-empty">No lines match.</div>`}</div>`;

  const search = host.querySelector<HTMLInputElement>("#backend-search");
  if (search) {
    search.value = backendSearch;
    search.addEventListener("change", () => {
      backendSearch = search.value;
      void loadBackend();
    });
  }
}

function visible(entries: readonly LogEntry[]): LogEntry[] {
  return entries.filter((e) => {
    if (!showPolled && POLLED_COMMANDS.has(e.command)) return false;
    if (!filter) return true;
    const haystack = `${e.command} ${JSON.stringify(e.args ?? "")} ${e.error ?? ""}`.toLowerCase();
    return haystack.includes(filter.toLowerCase());
  });
}

function drawList(entries: readonly LogEntry[]) {
  const list = root?.querySelector<HTMLElement>("#log-list");
  if (!list) return;
  const rows = visible(entries);

  if (rows.length === 0) {
    list.innerHTML = `<div class="dev-empty">No calls match.</div>`;
    return;
  }

  list.innerHTML = rows
    .map((e) => {
      const args = e.args ? JSON.stringify(e.args) : "";
      const detail =
        expanded === e.id
          ? output(e.ok ? { args: e.args, result: e.result } : { args: e.args, error: e.error }, !e.ok)
          : "";
      return `<div class="log-entry ${e.ok ? "ok" : "err"}" data-id="${e.id}">
        <span class="t">${clockTime(e.at)}</span>
        <span class="name">${escapeHtml(e.command)}<span class="args"> ${escapeHtml(
          args.length > 90 ? `${args.slice(0, 90)}…` : args,
        )}</span></span>
        <span class="ms">${e.ms.toFixed(0)} ms</span>
        <span class="st">${e.ok ? "ok" : "error"}</span>
      </div>${detail}`;
    })
    .join("");
}

export const logPanel: Panel = {
  id: "log",
  title: "Log",
  icon: "☰",
  group: "Tools",

  mount(el, ctx) {
    root = el;
    el.innerHTML =
      panelHead(
        "Log",
        "The backend's own log file — the one a release build writes too — and every IPC call this page has made.",
      ) +
      `<div class="row" style="margin-bottom:.8rem">
        <button type="button" class="chip ${source === "backend" ? "on" : ""}" data-source="backend">Backend</button>
        <button type="button" class="chip ${source === "ipc" ? "on" : ""}" data-source="ipc">Portal IPC</button>
      </div>
      <div id="backend-body" ${source === "backend" ? "" : "hidden"}></div>
      <div id="ipc-body" ${source === "ipc" ? "" : "hidden"}>
        <div class="row" style="margin-bottom:.8rem">
          <input type="search" id="log-filter" placeholder="Filter by command, args, or error…" style="flex:1 1 18rem" />
          <label class="check"><input type="checkbox" id="log-polled" /> Show 1 Hz polls</label>
          <button type="button" class="ghost" data-copy>Copy as JSON</button>
          <button type="button" class="ghost" data-clear>Clear</button>
        </div>
        <div class="log-list" id="log-list"></div>
      </div>`;

    el.addEventListener("click", (e) => {
      const target = e.target as HTMLElement;

      const sourceButton = target.closest<HTMLElement>("[data-source]");
      if (sourceButton) {
        source = sourceButton.dataset.source as Source;
        ctx.refresh();
        return;
      }
      if (source !== "backend") return;

      const level = target.closest<HTMLElement>("[data-level]")?.dataset.level;
      if (level) {
        // Never let the last one be turned off: an empty level set means
        // "no filter" on the Rust side, so unticking everything would show
        // the whole file rather than nothing, which reads as a bug.
        if (levels.has(level) && levels.size > 1) levels.delete(level);
        else levels.add(level);
        void loadBackend();
        return;
      }

      const tag = target.closest<HTMLElement>("[data-tag]")?.dataset.tag;
      if (tag) {
        if (hiddenTags.has(tag)) hiddenTags.delete(tag);
        else hiddenTags.add(tag);
        void loadBackend();
        return;
      }

      if (target.closest("[data-reload]")) {
        void loadBackend();
        return;
      }
      if (target.closest("[data-reveal]")) {
        void tryCall("dev_open_data_dir", { which: "app_data" });
      }
    });

    el.addEventListener("change", (e) => {
      const select = (e.target as HTMLElement).closest<HTMLSelectElement>("#log-file");
      if (!select) return;
      backendFile = select.value;
      void loadBackend();
    });

    if (source === "backend") void loadBackend();

    const filterInput = el.querySelector<HTMLInputElement>("#log-filter")!;
    const polledInput = el.querySelector<HTMLInputElement>("#log-polled")!;
    filterInput.value = filter;
    polledInput.checked = showPolled;

    filterInput.addEventListener("input", () => {
      filter = filterInput.value;
      drawList(logEntries());
    });
    polledInput.addEventListener("change", () => {
      showPolled = polledInput.checked;
      drawList(logEntries());
    });

    el.querySelector("[data-clear]")!.addEventListener("click", () => {
      clearLog();
      toast("Log cleared");
    });
    el.querySelector("[data-copy]")!.addEventListener("click", async () => {
      await navigator.clipboard.writeText(JSON.stringify(visible(logEntries()), null, 2));
      toast("Copied to clipboard", "ok");
    });

    el.querySelector("#log-list")!.addEventListener("click", (e) => {
      const entry = (e.target as HTMLElement).closest<HTMLElement>(".log-entry");
      if (!entry) return;
      const id = Number(entry.dataset.id);
      expanded = expanded === id ? null : id;
      drawList(logEntries());
    });

    unsubscribe = onLog(drawList);
  },

  unmount() {
    unsubscribe?.();
    unsubscribe = null;
    root = null;
  },
};
