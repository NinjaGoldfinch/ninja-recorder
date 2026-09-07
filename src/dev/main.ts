/**
 * Dev portal bootstrap: hash router, the shared 1 Hz health poll, and the
 * top bar.
 *
 * Panels are plain objects with a `mount(root)` and an optional
 * `onHealth`. The poll runs once here and fans out, rather than each
 * panel starting its own timer — the backend answers it with a single
 * `dev_health` round trip, and a page with five independent pollers would
 * throw that away.
 */
import { listen } from "@tauri-apps/api/event";
import { tryCall } from "./ipc";
import { bytes, escapeHtml, pill, toast } from "./ui";
import type { DevEnvInfo, DevHealth } from "./types";

import { overviewPanel } from "./panels/overview";
import { commandsPanel } from "./panels/commands";
import { databasePanel } from "./panels/database";
import { seedPanel } from "./panels/seed";
import { simulatePanel } from "./panels/simulate";
import { recorderPanel } from "./panels/recorder";
import { retentionPanel } from "./panels/retention";
import { fixturesPanel } from "./panels/fixtures";
import { diagnosticsPanel } from "./panels/diagnostics";
import { logPanel } from "./panels/log";

export interface Panel {
  id: string;
  title: string;
  icon: string;
  group: string;
  /** Renders into `root`. Called on every navigation to this panel. */
  mount(root: HTMLElement, ctx: PanelContext): void | Promise<void>;
  /** Called on each poll tick while this panel is the visible one. */
  onHealth?(health: DevHealth): void;
  /** Called when the panel is navigated away from. */
  unmount?(): void;
}

export interface PanelContext {
  env: DevEnvInfo | null;
  /** Re-renders the current panel from scratch. */
  refresh(): void;
  /** Switches panels programmatically (the Fixtures → Simulate handoff). */
  navigate(id: string, payload?: unknown): void;
  /** Whatever `navigate` was called with, consumed on mount. */
  payload: unknown;
}

const PANELS: Panel[] = [
  overviewPanel,
  recorderPanel,
  simulatePanel,
  databasePanel,
  seedPanel,
  retentionPanel,
  fixturesPanel,
  commandsPanel,
  diagnosticsPanel,
  logPanel,
];

let env: DevEnvInfo | null = null;
let current: Panel | null = null;
let pendingPayload: unknown = null;
let pollTimer: number | null = null;

const main = () => document.querySelector<HTMLElement>("#dev-main")!;

const ctx: PanelContext = {
  get env() {
    return env;
  },
  refresh: () => {
    if (current) void mountPanel(current);
  },
  navigate: (id, payload) => {
    pendingPayload = payload ?? null;
    location.hash = `#/${id}`;
  },
  get payload() {
    const p = pendingPayload;
    pendingPayload = null;
    return p;
  },
};

function renderNav() {
  const nav = document.querySelector<HTMLElement>("#dev-nav")!;
  let lastGroup = "";
  nav.innerHTML = PANELS.map((panel) => {
    const header =
      panel.group !== lastGroup ? `<div class="dev-nav-group">${panel.group}</div>` : "";
    lastGroup = panel.group;
    return `${header}<a class="dev-nav-link" href="#/${panel.id}" data-panel="${panel.id}">
      <span class="dev-nav-icon">${panel.icon}</span>${panel.title}
    </a>`;
  }).join("");
}

function markActive(id: string) {
  document.querySelectorAll<HTMLElement>(".dev-nav-link").forEach((link) => {
    link.classList.toggle("active", link.dataset.panel === id);
  });
}

/**
 * Hands the next panel a listener-free `#dev-main`.
 *
 * Panels bind their delegated handlers to the element they are given and
 * have no way to unbind them — `unmount` gets no reference to it. Mounting
 * onto the *same* element every time therefore stacked a handler per
 * mount, and `refresh()` alone remounts on every `r`, every
 * `library-changed`, and every panel that calls it. Two live handlers turn
 * one click into two toggles, which is a no-op that renders once on the
 * way through: the Log panel's tag chips lit up and reverted within the
 * same frame. Worse, the stale handlers belong to *other* panels, and
 * generic hooks like `[data-reload]` and `[data-copy]` are not unique
 * across them.
 *
 * A shallow clone carries the id, class and tabindex across and no
 * listeners at all, so every panel starts clean without any of them having
 * to know that this is why.
 */
function freshMain(): HTMLElement {
  const host = main();
  const fresh = host.cloneNode(false) as HTMLElement;
  host.replaceWith(fresh);
  return fresh;
}

async function mountPanel(panel: Panel) {
  current?.unmount?.();
  current = panel;
  markActive(panel.id);
  document.title = `${panel.title} — ninja-recorder dev portal`;
  await panel.mount(freshMain(), ctx);
}

function route() {
  const id = location.hash.replace(/^#\/?/, "") || PANELS[0].id;
  const panel = PANELS.find((p) => p.id === id) ?? PANELS[0];
  void mountPanel(panel);
}

// --- Top bar ---------------------------------------------------------

function renderPills(health: DevHealth | null, error?: string) {
  const el = document.querySelector<HTMLElement>("#status-pills")!;
  if (!health) {
    el.innerHTML = pill("backend", error ? "unreachable" : "…", error ? "danger" : "");
    return;
  }

  const state = health.supervisor.state;
  el.innerHTML = [
    pill("state", state, state === "Recording" ? "ok" : "", state === "Recording"),
    health.is_recording ? pill("recorder", "capturing", "ok", true) : "",
    health.replay_running ? pill("replay", "running", "warn", true) : "",
    health.fixture_recording ? pill("fixtures", "capturing", "warn") : "",
    pill("library", `${health.counts.recordings}`),
    pill("used", bytes(health.total_bytes)),
    pill("free", bytes(health.free_bytes), health.free_bytes < 1024 ** 3 ? "danger" : ""),
  ]
    .filter(Boolean)
    .join("");
}

async function poll() {
  const result = await tryCall<DevHealth>("dev_health");
  if (result.ok) {
    renderPills(result.value);
    current?.onHealth?.(result.value);
  } else {
    renderPills(null, result.error);
  }
}

function setPolling(on: boolean) {
  if (pollTimer !== null) {
    clearInterval(pollTimer);
    pollTimer = null;
  }
  if (on) {
    void poll();
    pollTimer = window.setInterval(poll, 1000);
  }
}

// --- Boot ------------------------------------------------------------

window.addEventListener("DOMContentLoaded", async () => {
  renderNav();

  const result = await tryCall<DevEnvInfo>("dev_env_info");
  if (result.ok) {
    env = result.value;
    const dbPath = document.querySelector<HTMLElement>("#db-path")!;
    // Split at the last separator so only the directory half can shrink —
    // see `.dev-dbpath` in styles.css for why this isn't a CSS ellipsis.
    const cut = Math.max(env.db_path.lastIndexOf("/"), env.db_path.lastIndexOf("\\"));
    dbPath.innerHTML = `<span class="dir">${escapeHtml(env.db_path.slice(0, cut))}</span><span class="file">${escapeHtml(env.db_path.slice(cut))}</span>`;
    dbPath.title = `Every write on this page lands in ${env.db_path}`;
  } else {
    // Reaching dev.html in a build without the `devtools` feature is the
    // one failure mode worth spelling out — every panel would otherwise
    // just show an opaque "command not found".
    main().innerHTML = `<div class="warnbar warnbar-danger">
      <strong>Dev commands are not available in this build.</strong>
      Run the app with <code>npm run tauri:dev</code>, which passes
      <code>--features devtools</code>.
    </div><pre class="output output-error">${result.error}</pre>`;
    return;
  }

  window.addEventListener("hashchange", route);
  route();

  const toggle = document.querySelector<HTMLInputElement>("#poll-toggle")!;
  toggle.addEventListener("change", () => setPolling(toggle.checked));
  setPolling(toggle.checked);

  // The portal is usually driving these changes, but the supervisor emits
  // it too — a real game finishing while the portal is open should show up
  // here as well as in the main window.
  listen("library-changed", () => {
    if (current?.id === "database" || current?.id === "seed" || current?.id === "retention") {
      ctx.refresh();
    }
  }).catch((err) => console.warn("library-changed listener unavailable:", err));

  document.addEventListener("keydown", (e) => {
    if (e.key === "r" && (e.metaKey || e.ctrlKey)) return; // let reload through
    if (e.key === "r" && !isTyping()) {
      ctx.refresh();
      toast("Refreshed");
    }
  });
});

function isTyping(): boolean {
  const el = document.activeElement;
  return (
    el instanceof HTMLInputElement ||
    el instanceof HTMLTextAreaElement ||
    el instanceof HTMLSelectElement
  );
}
