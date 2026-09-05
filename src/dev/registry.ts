/**
 * Every command the portal can invoke, with enough shape to generate a
 * form for it.
 *
 * Hand-maintained, because Tauri has no runtime reflection over
 * `generate_handler!` and this project deliberately has no type codegen
 * (`specta`/`ts-rs`). The mitigation is `dev_registered_commands`, which
 * returns the Rust side's own list of production commands — the Commands
 * panel diffs the two and shows a banner when they disagree, so drift
 * becomes visible rather than silent.
 */

export type ArgKind = "string" | "number" | "boolean" | "json";

export interface ArgSpec {
  name: string;
  kind: ArgKind;
  /** Rendered under the input. Say what the backend does with it. */
  help?: string;
  optional?: boolean;
  default?: unknown;
}

export interface CommandSpec {
  name: string;
  group: string;
  /** `dev` commands are compiled out of shipped builds. */
  dev: boolean;
  /** Writes, deletes, or otherwise cannot simply be re-run. */
  danger?: boolean;
  description: string;
  args?: ArgSpec[];
}

export const COMMANDS: CommandSpec[] = [
  // --- Recorder ----------------------------------------------------
  {
    name: "start_recording",
    group: "Recorder",
    dev: false,
    danger: true,
    description:
      "Starts the active capture backend, after a free-space preflight. Races the state machine's own automatic start — the supervisor doesn't know about this call.",
  },
  {
    name: "stop_recording",
    group: "Recorder",
    dev: false,
    danger: true,
    description: "Stops capture and returns the path of the file produced.",
  },
  {
    name: "is_recording",
    group: "Recorder",
    dev: false,
    description: "Whether the backend believes it is capturing right now.",
  },

  // --- Library -----------------------------------------------------
  {
    name: "list_recordings",
    group: "Library",
    dev: false,
    description: "Every row in the VOD library, newest first.",
  },
  {
    name: "rescan_recordings",
    group: "Library",
    dev: false,
    danger: true,
    description:
      "Reconciles rows against the folder: drops rows whose file is gone, imports untracked .mp4/.mkv files. Deletes rows.",
  },
  {
    name: "get_recording_markers",
    group: "Library",
    dev: false,
    description: "Timeline markers for one recording, ordered by video time.",
    args: [{ name: "recordingId", kind: "number", help: "recordings.id" }],
  },
  {
    name: "get_recording_samples",
    group: "Library",
    dev: false,
    description:
      "Advantage-curve samples for one recording. An empty array means the recording predates sampling, not an error.",
    args: [{ name: "recordingId", kind: "number", help: "recordings.id" }],
  },
  {
    name: "set_pinned",
    group: "Library",
    dev: false,
    danger: true,
    description: "Pins or unpins a recording. Pinned rows are exempt from retention deletion.",
    args: [
      { name: "recordingId", kind: "number" },
      { name: "pinned", kind: "boolean", default: true },
    ],
  },

  // --- Disk --------------------------------------------------------
  {
    name: "get_disk_usage",
    group: "Disk",
    dev: false,
    description: "Total library bytes, recording count, and free space on the recordings volume.",
  },
  {
    name: "get_retention_policy",
    group: "Disk",
    dev: false,
    description: "The saved policy. null on either field means that dimension is unbounded.",
  },
  {
    name: "set_retention_policy",
    group: "Disk",
    dev: false,
    danger: true,
    description:
      "Saves the policy AND immediately enforces it — this deletes files. Use dev_retention_preview first.",
    args: [
      {
        name: "policy",
        kind: "json",
        default: { max_total_bytes: 53687091200, max_age_days: 30 },
      },
    ],
  },

  {
    name: "preview_retention_policy",
    group: "Disk",
    dev: false,
    description:
      "Dry run of enforcement under the given policy. Writes nothing — the safe counterpart to set_retention_policy.",
    args: [
      { name: "policy", kind: "json", default: { max_total_bytes: 53687091200, max_age_days: 30 } },
    ],
  },
  {
    name: "delete_recording",
    group: "Library",
    dev: false,
    danger: true,
    description: "Deletes one recording's row and its file on disk.",
    args: [{ name: "recordingId", kind: "number" }],
  },
  {
    name: "get_recordings_dir",
    group: "Disk",
    dev: false,
    description: "Absolute path of the recordings directory.",
  },
  {
    name: "open_recordings_folder",
    group: "Disk",
    dev: false,
    description: "Reveals the recordings directory in the OS file manager.",
  },
  {
    name: "get_ui_prefs",
    group: "Settings",
    dev: false,
    description: "Every key/value in the settings_kv store (theme, default sort, \u2026).",
  },
  {
    name: "set_ui_pref",
    group: "Settings",
    dev: false,
    danger: true,
    description: "Writes one UI preference. Unseeded store — a missing key means 'use the frontend default'.",
    args: [
      { name: "key", kind: "string", default: "theme" },
      { name: "value", kind: "string", default: "dark" },
    ],
  },

  // --- Audio -------------------------------------------------------
  {
    name: "get_audio_preset",
    group: "Audio",
    dev: false,
    description:
      "The audio capture preset. Unlike the settings_kv prefs, this is parsed and validated backend-side \u2014 it decides what gets recorded.",
  },
  {
    name: "set_audio_preset",
    group: "Audio",
    dev: false,
    danger: true,
    description:
      "Chooses what gets captured and how it is split across mp4 audio tracks. Track 0 is always the combined mix.",
    args: [
      {
        name: "preset",
        kind: "json",
        help: 'e.g. {"preset":"game_mic_discord"} or {"preset":"game"}',
        default: { preset: "game" },
      },
    ],
  },
  {
    name: "list_audio_inputs",
    group: "Audio",
    dev: false,
    description:
      "Audio input devices for the microphone picker, default first. Empty off Windows.",
  },
  {
    name: "extract_audio_track",
    group: "Audio",
    dev: false,
    description:
      "Extracts one audio stem to a cached sidecar so the review player can play it. Rejects track 0, which plays from the video itself.",
    args: [
      { name: "recordingPath", kind: "string" },
      { name: "trackIndex", kind: "number", default: 1 },
    ],
  },

  // --- League ------------------------------------------------------
  {
    name: "lcu_status",
    group: "League",
    dev: false,
    description:
      "One-shot LCU check: lockfile discovery, auth, gameflow phase, summoner. Infallible — failures come back in the `error` field.",
  },
  {
    name: "game_state_status",
    group: "League",
    dev: false,
    description: "Current supervisor state and the last finalized recording.",
  },

  // --- Dev: diagnostics -------------------------------------------
  {
    name: "dev_env_info",
    group: "Dev · Diagnostics",
    dev: true,
    description: "Build, platform, active recorder backend, and every resolved path.",
  },
  {
    name: "dev_health",
    group: "Dev · Diagnostics",
    dev: true,
    description: "Everything the Overview panel polls, in one round trip.",
  },
  {
    name: "dev_registered_commands",
    group: "Dev · Diagnostics",
    dev: true,
    description: "The Rust side's own list of production commands, for the drift check.",
  },
  {
    name: "dev_open_data_dir",
    group: "Dev · Diagnostics",
    dev: true,
    description: "Reveals one of the app's directories in the OS file manager.",
    args: [{ name: "which", kind: "string", default: "recordings", help: "recordings | app_data | fixtures | repo_fixtures" }],
  },

  // --- Dev: database ----------------------------------------------
  {
    name: "dev_schema",
    group: "Dev · Database",
    dev: true,
    description: "Live PRAGMA table_info for every browsable table, plus row counts.",
  },
  {
    name: "dev_table_page",
    group: "Dev · Database",
    dev: true,
    description: "A page of one table. Column names in order_by are validated against the schema.",
    args: [
      { name: "table", kind: "string", default: "recordings" },
      { name: "limit", kind: "number", optional: true, default: 100 },
      { name: "offset", kind: "number", optional: true, default: 0 },
      { name: "orderBy", kind: "string", optional: true, help: 'e.g. "started_at DESC"' },
    ],
  },
  {
    name: "dev_sql_query",
    group: "Dev · Database",
    dev: true,
    danger: true,
    description: "Arbitrary SQL against the live library database.",
    args: [{ name: "sql", kind: "string", default: "SELECT * FROM recordings LIMIT 20" }],
  },
  {
    name: "dev_insert_row",
    group: "Dev · Database",
    dev: true,
    danger: true,
    description: "Inserts one row, bypassing the typed API and its path upsert rule.",
    args: [
      { name: "table", kind: "string", default: "recordings" },
      { name: "values", kind: "json", default: {} },
    ],
  },
  {
    name: "dev_update_row",
    group: "Dev · Database",
    dev: true,
    danger: true,
    description: "Updates one row by id.",
    args: [
      { name: "table", kind: "string", default: "recordings" },
      { name: "id", kind: "number" },
      { name: "values", kind: "json", default: {} },
    ],
  },
  {
    name: "dev_delete_row",
    group: "Dev · Database",
    dev: true,
    danger: true,
    description:
      "Deletes one row by id. Without deleteFile, the next rescan re-imports the recording from its file.",
    args: [
      { name: "table", kind: "string", default: "recordings" },
      { name: "id", kind: "number" },
      { name: "deleteFile", kind: "boolean", optional: true, default: false },
    ],
  },
  {
    name: "dev_reset_db",
    group: "Dev · Database",
    dev: true,
    danger: true,
    description: "Empties every table and restores the default retention policy.",
    args: [{ name: "alsoClearFiles", kind: "boolean", default: false }],
  },

  // --- Dev: seeding ------------------------------------------------
  {
    name: "dev_seed_library",
    group: "Dev · Seed",
    dev: true,
    danger: true,
    description: "Generates recordings, markers, samples, and their files on disk.",
    args: [{ name: "spec", kind: "json", default: {} }],
  },
  {
    name: "dev_clear_seeded",
    group: "Dev · Seed",
    dev: true,
    danger: true,
    description: "Removes every seeded recording and file. Captured recordings are untouched.",
  },

  // --- Dev: retention ----------------------------------------------
  {
    name: "dev_retention_preview",
    group: "Dev · Retention",
    dev: true,
    description:
      "Dry run: exactly what enforcement would delete, and how many bytes it would free. Touches nothing.",
    args: [
      { name: "policy", kind: "json", optional: true, help: "omit to use the saved policy" },
      { name: "nowMillis", kind: "number", optional: true, help: "override the clock to test age rules" },
    ],
  },

  // --- Dev: simulation ---------------------------------------------
  {
    name: "dev_dispatch_state_event",
    group: "Dev · Simulate",
    dev: true,
    danger: true,
    description: "Feeds one event through the live supervisor. Really starts and stops the recorder.",
    args: [{ name: "event", kind: "json", default: { kind: "gameflow_phase", phase: "InProgress" } }],
  },
  {
    name: "dev_inject_snapshot",
    group: "Dev · Simulate",
    dev: true,
    description: "Pushes one Live Client Data payload through the real marker/sample pipeline.",
    args: [{ name: "snapshot", kind: "json", default: {} }],
  },
  {
    name: "dev_session_snapshot",
    group: "Dev · Simulate",
    dev: true,
    description: "The in-flight recording session — markers and samples accumulating right now.",
  },
  {
    name: "dev_replay_start",
    group: "Dev · Simulate",
    dev: true,
    danger: true,
    description: "Plays a scripted game at a speed multiplier.",
    args: [{ name: "spec", kind: "json", default: {} }],
  },
  {
    name: "dev_replay_stop",
    group: "Dev · Simulate",
    dev: true,
    description: "Aborts a running replay.",
  },
  {
    name: "dev_replay_status",
    group: "Dev · Simulate",
    dev: true,
    description: "Progress of the running replay.",
  },
  {
    name: "dev_lcu_get",
    group: "Dev · Simulate",
    dev: true,
    description: "Raw GET against any LCU path. Needs the League Client running.",
    args: [{ name: "path", kind: "string", default: "/lol-gameflow/v1/gameflow-phase" }],
  },
  {
    name: "dev_fetch_match_summary",
    group: "Dev · Simulate",
    dev: true,
    description:
      "Exercises lcu::match_data::fetch_match_summary — implemented and tested, but called from nowhere in the app.",
    args: [{ name: "gameId", kind: "number" }],
  },
  {
    name: "dev_live_client_probe",
    group: "Dev · Simulate",
    dev: true,
    description: "Raw allgamedata fetch. Only reachable while a game is running.",
  },

  // --- Dev: fixtures -----------------------------------------------
  {
    name: "dev_fixtures_state",
    group: "Dev · Fixtures",
    dev: true,
    description: "Capture flag, both fixture roots, and every fixture found under them.",
  },
  {
    name: "dev_fixture_read",
    group: "Dev · Fixtures",
    dev: true,
    description: "Reads one fixture. Confined to the two known fixture roots.",
    args: [{ name: "path", kind: "string" }],
  },
  {
    name: "dev_fixture_write",
    group: "Dev · Fixtures",
    dev: true,
    danger: true,
    description: "Saves a payload as a fixture under the capture directory.",
    args: [
      { name: "group", kind: "string", default: "live-client" },
      { name: "name", kind: "string" },
      { name: "contents", kind: "string" },
    ],
  },
  {
    name: "dev_set_fixture_recording",
    group: "Dev · Fixtures",
    dev: true,
    description: "Turns response capture on or off for the running process.",
    args: [{ name: "enabled", kind: "boolean", default: true }],
  },
];

/** Production commands the portal knows about, for the drift check. */
export function productionCommandNames(): string[] {
  return COMMANDS.filter((c) => !c.dev).map((c) => c.name);
}
