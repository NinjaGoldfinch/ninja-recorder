import { flushSync, mount, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { PANELS } from "../../dev/panels";

/**
 * **Every route renders something.**
 *
 * The portal has no visual tests and no Windows verification of its own, so
 * before #72 a panel that threw on mount would have shown an empty page and
 * nothing would have said so. That is the same class of failure WS4.4 shipped
 * with every gate green, which is why `main.boot.test.ts` exists for the app;
 * this is its opposite number for `dev.html`.
 *
 * It walks all eleven routes against a stubbed backend and asserts each panel
 * puts its own heading on the page. It is deliberately shallow: it proves a
 * panel mounts, reads what it asks for, and survives the answer.
 */

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => {}),
}));

/** Minimal, valid-shaped answers. A panel that reaches for a field this does
 *  not have is a panel reading something the backend does not promise. */
const ANSWERS: Record<string, unknown> = {
  dev_env_info: {
    db_path: "C:\\Users\\dev\\AppData\\ninja-recorder\\library.db",
    recordings_dir: "C:\\Videos",
    sample_mp4_present: true,
    devtools: true,
  },
  dev_health: {
    supervisor: { state: "Idle", recording_elapsed_s: null, last_finalized: null },
    session: null,
    counts: { recordings: 0, markers: 0, samples: 0 },
    policy: { max_total_bytes: null, max_age_days: null },
    total_bytes: 0,
    free_bytes: 500 * 1024 ** 3,
    is_recording: false,
    replay_running: false,
    fixture_recording: false,
  },
  lcu_status: { connected: false, summoner: null, phase: null, error: null },
  dev_schema: [
    {
      name: "recordings",
      row_count: 1,
      columns: [
        { name: "id", decl_type: "INTEGER", not_null: true, default_value: null, pk: true },
        { name: "champion", decl_type: "TEXT", not_null: false, default_value: null, pk: false },
      ],
    },
    {
      name: "markers",
      row_count: 1,
      columns: [
        { name: "id", decl_type: "INTEGER", not_null: true, default_value: null, pk: true },
        { name: "kind", decl_type: "TEXT", not_null: true, default_value: null, pk: false },
      ],
    },
  ],
  dev_table_page: {
    columns: ["id", "champion"],
    rows: [[1, "Ahri"]],
    rows_affected: 0,
    returned_rows: true,
    elapsed_ms: 0.5,
  },
  list_recordings: [
    {
      id: 12,
      path: "C:\\Videos\\a.mp4",
      started_at: 1_700_000_000_000,
      duration_s: 1500,
      size_bytes: 1024,
      champion: "Ahri",
      queue: 420,
      game_id: 999,
      pinned: false,
      result: "Win",
    },
  ],
  dev_recording_report: {
    row: { id: 12, champion: "Ahri", queue: 420, game_id: 999 },
    provenance: [{ field: "champion", source: "finalize", note: "from the poller" }],
    markers: 3,
    samples: 90,
    gold_samples: 90,
    alignment_offset_s: 1.25,
    scoreboard: null,
    diagnostics: null,
  },
  dev_backfill_recording: { matched: 1, skipped: 0 },
  dev_sql_query: {
    columns: ["n"],
    rows: [[1]],
    rows_affected: 0,
    returned_rows: true,
    elapsed_ms: 1.234,
  },
  dev_recording_vs_lcu: {
    game_id: 999,
    differing: 1,
    fields: [{ field: "champion", stored: "Ahri", live: "Sylas", verdict: "differ" }],
  },
  dev_inject_snapshot: {
    accepted: true,
    state: "Recording",
    markers_added: 2,
    samples_added: 1,
    note: "",
    session: null,
  },
  dev_set_fixture_recording: true,
  dev_dispatch_state_event: {
    before: { state: "Idle" },
    after: { state: "ClientRunning" },
    session: null,
  },
  dev_seed_library: {
    recording_ids: [1],
    markers_inserted: 10,
    samples_inserted: 20,
    bytes_written: 1024,
    used_sample_mp4: false,
    paths: ["C:\\Videos\\seed-1.mp4"],
  },
  dev_log_files: [],
  dev_read_log: {
    file: "daemon.log",
    dir: "C:\\logs",
    lines: [],
    tags_present: [],
    matched: 0,
    total: 0,
    truncated: false,
  },
  dev_fixtures_state: {
    recording_enabled: false,
    capture_dir: "C:\\fixtures",
    repo_dir: null,
    entries: [],
  },
  dev_replay_status: {
    running: false,
    finished: false,
    game_time_s: 0,
    duration_s: 0,
    ticks: 0,
    events_fired: 0,
    error: null,
  },
  dev_retention_preview: {
    policy: { max_total_bytes: null, max_age_days: null },
    now_millis: 1_700_000_000_000,
    total_bytes: 0,
    pinned_bytes: 0,
    to_delete: [],
    would_free_bytes: 0,
    total_after_bytes: 0,
  },
};

/** Flipped by the one test about a build without the `devtools` feature. */
let envFails = false;

const tryCall = vi.fn(async (command: string, _args?: Record<string, unknown>) => {
  if (command === "dev_env_info" && envFails) {
    return { ok: false as const, error: "command dev_env_info not found" };
  }
  return command in ANSWERS
    ? { ok: true as const, value: ANSWERS[command] }
    : { ok: false as const, error: `no stub for ${command}` };
});

const call = vi.fn(async (command: string, _args?: Record<string, unknown>) => ANSWERS[command]);

vi.mock("../../../dev/ipc", () => ({
  call: (command: string, args?: Record<string, unknown>) => call(command, args),
  tryCall: (command: string, args?: Record<string, unknown>) => tryCall(command, args),
  onLog: () => () => {},
  logEntries: () => [],
  clearLog: vi.fn(),
  POLLED_COMMANDS: new Set(["dev_health", "dev_replay_status"]),
}));

let host: HTMLElement;
let instance: Record<string, unknown> | null = null;
let DevApp: typeof import("./DevApp.svelte").default;

beforeEach(async () => {
  vi.useFakeTimers();
  DevApp = (await import("./DevApp.svelte")).default;
  host = document.createElement("div");
  document.body.append(host);
});

afterEach(async () => {
  if (instance) await unmount(instance, { outro: false });
  host?.remove();
  instance = null;
  envFails = false;
  tryCall.mockClear();
  call.mockClear();
  location.hash = "";
  vi.useRealTimers();
});

/** Mounts, then lets every load effect and its awaited answer land. */
async function open(hash: string) {
  location.hash = hash;
  if (!instance) {
    instance = mount(DevApp, { target: host });
  } else {
    window.dispatchEvent(new HashChangeEvent("hashchange"));
  }
  flushSync();
  // Each panel awaits at least one round trip before it has anything to show,
  // and Library chains a second one off the first. Turning the microtask queue
  // over generously is cheaper than counting awaits per panel.
  for (let i = 0; i < 25; i++) await Promise.resolve();
  flushSync();
}

/** Lets a handler's awaits and the render they cause land. */
async function settle() {
  for (let i = 0; i < 25; i++) await Promise.resolve();
  flushSync();
}

/** Clicks the first button whose text contains `label`. Fails loudly rather
 *  than silently doing nothing, which is how a renamed button would otherwise
 *  turn an interaction test into a test of nothing. */
async function press(label: string) {
  const button = [...host.querySelectorAll("button")].find((b) => b.textContent?.includes(label));
  if (!button) throw new Error(`no button matching ${JSON.stringify(label)}`);
  button.click();
  await settle();
}

describe("the dev portal", () => {
  it("mounts the shell", async () => {
    await open("#/overview");
    expect(host.querySelector(".dev-topbar")).not.toBeNull();
    expect(host.querySelector(".dev-sidebar")).not.toBeNull();
    expect(host.querySelectorAll(".dev-nav-link")).toHaveLength(PANELS.length);
  });

  it.each(PANELS.map((p) => [p.id, p.title] as const))(
    "renders the %s panel",
    async (id, title) => {
      await open(`#/${id}`);
      const head = host.querySelector(".panel-head h1");
      expect(head?.textContent?.trim(), `${id} rendered no heading`).toBe(title);
    },
  );

  it("titles the window after the panel, so a second portal is distinguishable", async () => {
    await open("#/database");
    expect(document.title).toContain("Database");
  });

  it("marks the open panel in the sidebar", async () => {
    await open("#/seed");
    const active = host.querySelector(".dev-nav-link.active");
    expect(active?.textContent).toContain("Seed");
  });

  it("falls back to the first panel for an unknown route", async () => {
    await open("#/nonsense");
    expect(host.querySelector(".panel-head h1")?.textContent?.trim()).toBe(PANELS[0].title);
  });

  it("opens Library on the recording a deep link names", async () => {
    await open("#/library/12");
    expect(tryCall).toHaveBeenCalledWith("dev_recording_report", { recordingId: 12 });
  });

  it("says what to run when the build has no dev commands", async () => {
    envFails = true;
    await open("#/overview");
    expect(host.textContent).toContain("Dev commands are not available in this build");
    expect(host.textContent).toContain("npm run tauri:dev");
  });
});

describe("the panels that write", () => {
  it("inserts a row with the values typed, and never the primary key", async () => {
    await open("#/database");
    await press("New row");

    const fields = host.querySelectorAll<HTMLInputElement>(".field-grid input");
    expect(fields).toHaveLength(2);
    // The pk is shown and disabled: changing it would orphan the markers and
    // samples that reference it.
    expect(fields[0].disabled).toBe(true);
    fields[1].value = "Ahri";
    fields[1].dispatchEvent(new Event("input", { bubbles: true }));
    await settle();

    await press("Insert");
    expect(call).toHaveBeenCalledWith("dev_insert_row", {
      table: "recordings",
      values: { champion: "Ahri" },
    });
  });

  it("opens a row into the editor and updates it by id", async () => {
    await open("#/database");
    host.querySelector<HTMLTableRowElement>("tbody tr")?.click();
    await settle();

    await press("Save changes");
    expect(call).toHaveBeenCalledWith("dev_update_row", {
      table: "recordings",
      id: 1,
      values: { champion: "Ahri" },
    });
  });

  it("asks before deleting a row, and writes nothing if the answer is no", async () => {
    await open("#/database");
    host.querySelector<HTMLTableRowElement>("tbody tr")?.click();
    await settle();
    await press("Delete row");

    expect(host.querySelector(".modal")).not.toBeNull();
    await press("Cancel");
    expect(call).not.toHaveBeenCalledWith("dev_delete_row", expect.anything());
  });

  it("deletes the file along with a recordings row", async () => {
    await open("#/database");
    host.querySelector<HTMLTableRowElement>("tbody tr")?.click();
    await settle();
    await press("Delete row");
    host.querySelector<HTMLButtonElement>(".modal .danger")?.click();
    await settle();

    expect(call).toHaveBeenCalledWith("dev_delete_row", {
      table: "recordings",
      id: 1,
      deleteFile: true,
    });
  });

  it("never offers to delete a file for a table that has none", async () => {
    await open("#/database");
    await press("markers");
    host.querySelector<HTMLTableRowElement>("tbody tr")?.click();
    await settle();

    expect(host.textContent).not.toContain("also delete the file");
    await press("Delete row");
    host.querySelector<HTMLButtonElement>(".modal .danger")?.click();
    await settle();

    expect(call).toHaveBeenCalledWith("dev_delete_row", {
      table: "markers",
      id: 1,
      deleteFile: false,
    });
  });

  it("holds the reset behind typing the word", async () => {
    await open("#/database");
    await press("Reset database");

    const confirm = host.querySelector<HTMLButtonElement>(".modal .danger");
    expect(confirm?.disabled, "reset was armed before anything was typed").toBe(true);

    const input = host.querySelector<HTMLInputElement>(".modal input");
    if (input) {
      input.value = "reset";
      input.dispatchEvent(new Event("input", { bubbles: true }));
    }
    await settle();
    expect(host.querySelector<HTMLButtonElement>(".modal .danger")?.disabled).toBe(false);
  });

  it("seeds with the preset that is selected, not the one that was default", async () => {
    await open("#/seed");
    await press("Retention: size");
    await press("Seed ");

    const [, payload] = tryCall.mock.calls.find(([c]) => c === "dev_seed_library") ?? [];
    const spec = (payload as { spec: { count: number; pinned_every: number } }).spec;
    expect(spec.count).toBe(20);
    expect(spec.pinned_every).toBe(4);
  });

  it("invokes a command with no arguments as no payload at all", async () => {
    await open("#/commands");
    // Chosen by name: the list is alphabetical, so whichever command happens
    // to come first is not guaranteed to take no arguments.
    await press("is_recording");
    await press("Invoke");
    // `undefined` rather than `{}`: an empty object is a claim that the
    // command takes arguments.
    expect(call).toHaveBeenCalledWith("is_recording", undefined);
  });

  it("dispatches a state event into the live supervisor", async () => {
    await open("#/simulate");
    await press("Client opened");
    expect(tryCall).toHaveBeenCalledWith("dev_dispatch_state_event", {
      event: { kind: "lockfile_present" },
    });
    expect(host.textContent).toContain("Idle → ClientRunning");
  });

  it("re-reads the report after an action that writes", async () => {
    await open("#/library/12");
    tryCall.mockClear();
    await press("Backfill this row");

    expect(tryCall).toHaveBeenCalledWith("dev_backfill_recording", { recordingId: 12 });
    // The whole panel exists to be trusted about what a row currently says.
    expect(tryCall).toHaveBeenCalledWith("dev_recording_report", { recordingId: 12 });
  });

  it("will not run the deferred patch on a row with no game", async () => {
    ANSWERS.dev_recording_report = {
      ...(ANSWERS.dev_recording_report as object),
      row: { id: 12, champion: "Ahri", queue: 420, game_id: null },
    };
    await open("#/library/12");

    const patch = [...host.querySelectorAll("button")].find((b) =>
      b.textContent?.includes("Re-run the deferred patch"),
    );
    expect(patch?.disabled).toBe(true);
    expect(host.textContent).toContain("The deferred patch is unavailable");
  });

  it("narrows the backend log query as levels are turned off", async () => {
    await open("#/log");
    await press("DEBUG");

    const [, payload] = [...tryCall.mock.calls].reverse().find(([c]) => c === "dev_read_log") ?? [];
    const query = (payload as { query: { levels: string[] } }).query;
    expect(query.levels).not.toContain("DEBUG");
    expect(query.levels).toContain("ERROR");
  });
});

describe("the panels that read something back", () => {
  it("runs a query and says how long it took", async () => {
    await open("#/database");
    const sql = host.querySelector<HTMLTextAreaElement>("textarea");
    if (!sql) throw new Error("no SQL editor");
    sql.value = "SELECT 1 AS n";
    sql.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();

    await press("Run");
    expect(tryCall).toHaveBeenCalledWith("dev_sql_query", { sql: "SELECT 1 AS n" });
    expect(host.querySelector(".output")?.textContent).toContain("1.23");
  });

  it("re-reads the browser after a query, because a write changes what it shows", async () => {
    await open("#/database");
    const sql = host.querySelector<HTMLTextAreaElement>("textarea");
    if (sql) {
      sql.value = "DELETE FROM markers";
      sql.dispatchEvent(new Event("input", { bubbles: true }));
    }
    await settle();
    tryCall.mockClear();
    await press("Run");

    expect(tryCall).toHaveBeenCalledWith("dev_schema", undefined);
  });

  it("refuses to run an empty query rather than asking the backend", async () => {
    await open("#/database");
    tryCall.mockClear();
    await press("Run");
    expect(tryCall).not.toHaveBeenCalledWith("dev_sql_query", expect.anything());
  });

  it("filters the command list as you search", async () => {
    await open("#/commands");
    const before = host.querySelectorAll(".cmd-item").length;
    const search = host.querySelector<HTMLInputElement>('input[type="search"]');
    if (!search) throw new Error("no search box");
    search.value = "retention";
    search.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();

    const after = host.querySelectorAll(".cmd-item").length;
    expect(after).toBeGreaterThan(0);
    expect(after).toBeLessThan(before);
  });

  it("leads with the count of what disagrees when the client is asked", async () => {
    await open("#/library/12");
    await press("Ask the client");

    expect(tryCall).toHaveBeenCalledWith("dev_recording_vs_lcu", { recordingId: 12 });
    expect(host.textContent).toContain("1 field");
    expect(host.querySelector(".verdict-differ")).not.toBeNull();
  });

  it("says a snapshot is not JSON before asking the backend to take it", async () => {
    await open("#/simulate");
    const box = host.querySelector<HTMLTextAreaElement>("textarea");
    if (box) {
      box.value = "{not json";
      box.dispatchEvent(new Event("input", { bubbles: true }));
    }
    await settle();
    await press("Inject");

    expect(tryCall).not.toHaveBeenCalledWith("dev_inject_snapshot", expect.anything());
    expect(host.textContent).toContain("Not valid JSON");
  });

  it("injects a parsed snapshot and reports what it added", async () => {
    await open("#/simulate");
    const box = host.querySelector<HTMLTextAreaElement>("textarea");
    if (box) {
      box.value = '{"gameData":{"gameTime":92}}';
      box.dispatchEvent(new Event("input", { bubbles: true }));
    }
    await settle();
    await press("Inject");

    expect(tryCall).toHaveBeenCalledWith("dev_inject_snapshot", {
      snapshot: { gameData: { gameTime: 92 } },
    });
    expect(host.textContent).toContain("+2 marker(s)");
  });

  it("will not start a replay with no base payload to rewrite", async () => {
    await open("#/simulate");
    await press("Start replay");
    expect(tryCall).not.toHaveBeenCalledWith("dev_replay_start", expect.anything());
  });

  it("turns fixture capture on through the command, not a local flag", async () => {
    await open("#/fixtures");
    const box = host.querySelector<HTMLInputElement>('.dev-main input[type="checkbox"]');
    if (!box) throw new Error("no capture toggle");
    box.checked = true;
    box.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();

    expect(tryCall).toHaveBeenCalledWith("dev_set_fixture_recording", { enabled: true });
  });

  it("switches the Log panel between the two logs", async () => {
    await open("#/log");
    expect(host.textContent).toContain("hidden by default");
    await press("Portal IPC");
    expect(host.textContent).toContain("No calls match.");
  });
});
