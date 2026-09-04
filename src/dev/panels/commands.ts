/**
 * IPC console: every command, an argument form generated from the
 * registry, and the raw response.
 *
 * This is the answer to "test every backend feature" — anything reachable
 * over `invoke` is reachable here, including the commands no other panel
 * bothers to surface.
 */
import type { Panel, PanelContext } from "../main";
import { call, tryCall } from "../ipc";
import { COMMANDS, productionCommandNames, type ArgSpec, type CommandSpec } from "../registry";
import { card, escapeHtml, output, panelHead, toast } from "../ui";

let root: HTMLElement | null = null;
let selected: CommandSpec = COMMANDS[0];
let search = "";
let drift: string[] = [];
let result: { value: unknown; ms: number; error: boolean } | null = null;
/** Last argument values per command, so re-running one is one click. */
const remembered = new Map<string, Record<string, string>>();

function matching(): CommandSpec[] {
  const q = search.trim().toLowerCase();
  if (!q) return COMMANDS;
  return COMMANDS.filter(
    (c) => c.name.includes(q) || c.group.toLowerCase().includes(q) || c.description.toLowerCase().includes(q),
  );
}

function listHtml(): string {
  let group = "";
  return matching()
    .map((c) => {
      const header = c.group !== group ? `<div class="dev-nav-group">${escapeHtml(c.group)}</div>` : "";
      group = c.group;
      const cls = [
        "cmd-item",
        c.name === selected.name ? "active" : "",
        c.danger ? "is-danger" : c.dev ? "is-dev" : "",
      ]
        .filter(Boolean)
        .join(" ");
      return `${header}<button type="button" class="${cls}" data-cmd="${c.name}">
        <span class="dot"></span>${escapeHtml(c.name)}
      </button>`;
    })
    .join("");
}

/** One field. The help text lives *inside* the label — as a sibling it
 *  would land in its own `.field-grid` cell, ending up beside an unrelated
 *  field rather than under its own. */
function argField(spec: ArgSpec, saved: string | undefined): string {
  const value = saved ?? (spec.default === undefined ? "" : stringify(spec.default));
  const label = `${spec.name}${spec.optional ? " (optional)" : ""}`;
  const help = spec.help ? `<span class="hint">${escapeHtml(spec.help)}</span>` : "";

  if (spec.kind === "boolean") {
    return `<div class="field">
      <label class="check">
        <input type="checkbox" data-arg="${spec.name}" data-kind="boolean" ${
          value === "true" ? "checked" : ""
        } />
        ${escapeHtml(label)}
      </label>${help}
    </div>`;
  }
  if (spec.kind === "json") {
    return `<label class="field"><span>${escapeHtml(label)} — JSON</span>
      <textarea rows="5" data-arg="${spec.name}" data-kind="json" spellcheck="false">${escapeHtml(
        value,
      )}</textarea>${help}
    </label>`;
  }
  return `<label class="field"><span>${escapeHtml(label)}</span>
    <input type="${spec.kind === "number" ? "number" : "text"}" data-arg="${spec.name}"
      data-kind="${spec.kind}" value="${escapeHtml(value)}" spellcheck="false" />${help}
  </label>`;
}

function stringify(value: unknown): string {
  return typeof value === "object" && value !== null
    ? JSON.stringify(value, null, 2)
    : String(value);
}

function detailHtml(): string {
  const args = selected.args ?? [];
  const saved = remembered.get(selected.name) ?? {};

  return card(
    selected.name,
    `<p class="hint-block" style="margin-top:0">${escapeHtml(selected.description)}</p>
     ${
       selected.danger
         ? `<div class="warnbar warnbar-danger">This command writes, deletes, or otherwise
            changes state. It is not safely repeatable.</div>`
         : ""
     }
     ${
       args.length
         ? `<div class="field-grid" style="margin-top:.8rem">${args
             .map((a) => argField(a, saved[a.name]))
             .join("")}</div>`
         : `<p class="hint">No arguments.</p>`
     }
     <div class="row" style="margin-top:.9rem">
       <button type="button" class="primary" data-invoke>Invoke</button>
       ${result ? `<button type="button" class="ghost" data-copy>Copy response</button>` : ""}
       ${result ? `<span class="hint num">${result.ms.toFixed(1)} ms</span>` : ""}
     </div>
     ${result ? output(result.value, result.error) : ""}`,
    "",
    true,
  );
}

function draw() {
  if (!root) return;
  root.innerHTML =
    panelHead(
      "Commands",
      "Every command the backend exposes, with a generated argument form. Blue dots are dev-only commands; red dots change state.",
    ) +
    (drift.length
      ? `<div class="warnbar warnbar-danger"><strong>Command registry drift.</strong>
         The Rust handler list and this page's registry disagree about: ${escapeHtml(
           drift.join(", "),
         )}. One of <code>src-tauri/src/lib.rs</code>,
         <code>dev::dev_registered_commands</code>, or <code>src/dev/registry.ts</code> is stale.</div>`
      : "") +
    `<div class="split">
      <div>
        <input type="search" id="cmd-search" placeholder="Search commands…" style="width:100%;margin-bottom:.5rem" />
        <div class="cmd-list" id="cmd-list">${listHtml()}</div>
      </div>
      <div id="cmd-detail">${detailHtml()}</div>
    </div>`;

  const searchInput = root.querySelector<HTMLInputElement>("#cmd-search")!;
  searchInput.value = search;
  searchInput.addEventListener("input", () => {
    search = searchInput.value;
    root!.querySelector("#cmd-list")!.innerHTML = listHtml();
  });
}

function collectArgs(): Record<string, unknown> | null {
  const args: Record<string, unknown> = {};
  const saved: Record<string, string> = {};

  for (const spec of selected.args ?? []) {
    const el = root!.querySelector<HTMLInputElement | HTMLTextAreaElement>(
      `[data-arg="${spec.name}"]`,
    );
    if (!el) continue;

    if (spec.kind === "boolean") {
      const checked = (el as HTMLInputElement).checked;
      saved[spec.name] = String(checked);
      args[spec.name] = checked;
      continue;
    }

    const raw = el.value.trim();
    saved[spec.name] = el.value;

    // An omitted optional argument must be absent, not null — several
    // commands distinguish "not given" (use the saved value) from an
    // explicit null.
    if (raw === "") {
      if (spec.optional) continue;
      toast(`${spec.name} is required`, "err");
      return null;
    }

    if (spec.kind === "number") {
      const n = Number(raw);
      if (Number.isNaN(n)) {
        toast(`${spec.name} is not a number`, "err");
        return null;
      }
      args[spec.name] = n;
    } else if (spec.kind === "json") {
      try {
        args[spec.name] = JSON.parse(raw);
      } catch (err) {
        toast(`${spec.name} is not valid JSON: ${err}`, "err");
        return null;
      }
    } else {
      args[spec.name] = raw;
    }
  }

  remembered.set(selected.name, saved);
  return args;
}

async function invokeSelected() {
  const args = collectArgs();
  if (!args) return;

  const started = performance.now();
  try {
    const value = await call<unknown>(selected.name, Object.keys(args).length ? args : undefined);
    result = { value: value ?? "(no value returned)", ms: performance.now() - started, error: false };
  } catch (err) {
    result = { value: String(err), ms: performance.now() - started, error: true };
  }
  root!.querySelector("#cmd-detail")!.innerHTML = detailHtml();
}

export const commandsPanel: Panel = {
  id: "commands",
  title: "Commands",
  icon: "⌘",
  group: "Tools",

  async mount(el, _ctx: PanelContext) {
    root = el;
    result = null;
    draw();

    const registered = await tryCall<string[]>("dev_registered_commands");
    if (registered.ok) {
      const rust = new Set(registered.value);
      const ts = new Set(productionCommandNames());
      drift = [
        ...[...rust].filter((n) => !ts.has(n)).map((n) => `${n} (missing from this page)`),
        ...[...ts].filter((n) => !rust.has(n)).map((n) => `${n} (not registered in Rust)`),
      ];
      if (drift.length) draw();
    }

    el.addEventListener("click", async (e) => {
      const target = e.target as HTMLElement;

      const item = target.closest<HTMLElement>("[data-cmd]");
      if (item) {
        selected = COMMANDS.find((c) => c.name === item.dataset.cmd) ?? selected;
        result = null;
        el.querySelector("#cmd-list")!.innerHTML = listHtml();
        el.querySelector("#cmd-detail")!.innerHTML = detailHtml();
        return;
      }

      if (target.closest("[data-invoke]")) {
        await invokeSelected();
        return;
      }

      if (target.closest("[data-copy]") && result) {
        await navigator.clipboard.writeText(
          typeof result.value === "string" ? result.value : JSON.stringify(result.value, null, 2),
        );
        toast("Copied to clipboard", "ok");
      }
    });
  },

  unmount() {
    root = null;
  },
};
