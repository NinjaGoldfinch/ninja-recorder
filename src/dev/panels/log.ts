/**
 * Every IPC call the portal has made this session.
 *
 * The backend logs to stdout with `println!`, which is only visible in the
 * terminal running `tauri dev`; the frontend logged nothing at all before
 * this. Polled commands are hidden by default — at 1 Hz they bury
 * everything a person actually clicked within seconds.
 */
import type { Panel } from "../main";
import { clearLog, logEntries, onLog, POLLED_COMMANDS, type LogEntry } from "../ipc";
import { clockTime, escapeHtml, output, panelHead, toast } from "../ui";

let root: HTMLElement | null = null;
let unsubscribe: (() => void) | null = null;
let showPolled = false;
let filter = "";
let expanded: number | null = null;

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

  mount(el) {
    root = el;
    el.innerHTML =
      panelHead(
        "Log",
        "Every backend call this page has made, newest first. Click one to see its arguments and result.",
      ) +
      `<div class="row" style="margin-bottom:.8rem">
        <input type="search" id="log-filter" placeholder="Filter by command, args, or error…" style="flex:1 1 18rem" />
        <label class="check"><input type="checkbox" id="log-polled" /> Show 1 Hz polls</label>
        <button type="button" class="ghost" data-copy>Copy as JSON</button>
        <button type="button" class="ghost" data-clear>Clear</button>
      </div>
      <p class="hint-block" style="margin-top:0">Backend logging is <code>println!</code> to
       stdout — look at the terminal running <code>npm run tauri:dev</code> for
       <code>[db]</code>, <code>[retention]</code>, <code>[recorder]</code> and
       <code>[state_machine]</code> lines.</p>
      <div class="log-list" id="log-list"></div>`;

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
