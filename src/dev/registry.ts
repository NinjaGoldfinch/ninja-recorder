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
  {
    name: "get_autostart",
    group: "Settings",
    dev: false,
    description:
      "Whether the app is registered to start on login, read live from the platform (HKCU\\...\\Run on Windows) rather than from settings_kv. `supported: false` means this build has no autostart control.",
  },
  {
    name: "set_autostart",
    group: "Settings",
    dev: false,
    danger: true,
    description:
      "Adds or removes the login entry for this executable, then returns what the platform says afterwards \u2014 which is not always what was asked for. Writes outside the app's own data: enabling here really does register the running binary, dev build included.",
    args: [{ name: "enabled", kind: "boolean", default: false }],
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
  {
    name: "dev_recording_report",
    group: "Dev · Diagnostics",
    dev: true,
    description:
      "One recording's whole story: the row, a named likely writer for every field that has more than one, marker and sample counts, the alignment offset, and the scoreboard and diagnostics parsed. Read-only \u2014 the actions that operate on a recording are their own commands.",
    args: [{ name: "recordingId", kind: "number" }],
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
    name: "dev_log_files",
    group: "Dev · Tools",
    dev: true,
    description:
      "The backend log files that exist, newest first, including the rotated ones. Reports the missing ones too — \"no log file\" and \"empty log file\" are different answers.",
  },
  {
    name: "dev_read_log",
    group: "Dev · Tools",
    dev: true,
    description:
      "One filtered window of a log file, newest matching lines first. Filtering happens in Rust: the file is capped at 5 MiB, which is far too much to hand a webview whole.",
    args: [
      {
        name: "query",
        kind: "json",
        default: { file: null, levels: [], hideTags: ["live-poll"], search: "", limit: 200 },
        help: "levels includes; hideTags excludes. Empty levels means every level, not none.",
      },
    ],
  },
  {
    name: "backfill_match_metadata",
    group: "Library",
    dev: false,
    description:
      "Matches every recording with no champion or result against the client's match history, by when it was played. Refuses a recording that overlaps more than one game rather than guessing. Needs the League Client running.",
    args: [],
  },
  {
    name: "resolve_icons",
    group: "Library",
    dev: false,
    description:
      "Cached Data Dragon art for a page of rows — champions by display name, items and runes by id, spells by display name. Fetches whatever is not cached yet. Anything that could not be resolved is absent from the result rather than null.",
    args: [
      {
        name: "request",
        kind: "json",
        default: { champions: ["Wukong"], items: [3089], spells: ["Flash"], runes: [8112] },
        help: "Four lists: champions, items, spells, runes. Any of them may be omitted.",
      },
    ],
  },
  {
    name: "dev_trim_lead_in",
    group: "Dev · Tools",
    dev: true,
    danger: true,
    description:
      "Cuts the loading screen off a recording's file and rebases its markers and samples onto what is left. Finalize already does this; the command is for recordings made before it did, or one it skipped. A no-op on anything already trimmed.",
    args: [
      {
        name: "recordingId",
        kind: "number",
        help: "The row to cut. It needs samples: the loading screen's length is measured from them.",
      },
    ],
  },
  {
    name: "dev_lcu_get",
    group: "Dev · Simulate",
    dev: true,
    description: "Raw GET against any LCU path. Needs the League Client running.",
    args: [{ name: "path", kind: "string", default: "/lol-gameflow/v1/gameflow-phase" }],
  },
  {
    name: "dev_champion_name",
    group: "Dev · Simulate",
    dev: true,
    description:
      "Resolves a champion id through the real asset-store lookup. 62 must come back as Wukong — MonkeyKing means the parse is reading `alias`. Needs the League Client running.",
    args: [
      {
        name: "championId",
        kind: "number",
        default: 62,
        help: "62 is the one worth asking: its display name and its alias differ.",
      },
    ],
  },
  {
    name: "dev_fetch_match_summary",
    group: "Dev · Simulate",
    dev: true,
    description:
      "One shot at the post-game summary — end-of-game block, then match history — with no retries. Needs the League Client running.",
    args: [{ name: "gameId", kind: "number" }],
  },
  {
    name: "dev_patch_match_summary",
    group: "Dev · Simulate",
    dev: true,
    danger: true,
    description:
      "Runs the whole deferred patch against a real client and writes the result to an existing recording. May block for up to a minute — that is the real retry schedule.",
    args: [
      { name: "recordingId", kind: "number", help: "The row to patch. Its queue/role/patch columns are overwritten." },
      { name: "gameId", kind: "number", help: "Which game to ask the client about." },
      {
        name: "isCustom",
        kind: "boolean",
        default: false,
        help: "Skips match history — a custom game never reaches it.",
      },
    ],
  },
  {
    name: "dev_live_client_probe",
    group: "Dev · Simulate",
    dev: true,
    description: "Raw allgamedata fetch. Only reachable while a game is running.",
  },

  // --- Updates ------------------------------------------------------
  {
    name: "get_update_status",
    group: "Updates",
    dev: false,
    description:
      "What the last background check found, with installability recomputed against live state. `unsupported` in a devtools build \u2014 this one \u2014 because the update seam is never wired there.",
  },
  {
    name: "check_for_update",
    group: "Updates",
    dev: false,
    description:
      "Asks for a check now rather than waiting for the six-hourly one. Returns as soon as the request is handed over; the answer arrives on the `update-status-changed` event. Refuses in a devtools build.",
  },
  {
    name: "install_update",
    group: "Updates",
    dev: false,
    danger: true,
    description:
      "Downloads the offered installer and hands the machine over to it \u2014 this ends the process. Refuses while anything is being recorded, and refuses outright in a devtools build.",
  },

  // --- Dev: fixtures -----------------------------------------------
  {
    name: "dev_fixtures_state",
    group: "Dev · Fixtures",
    dev: true,
    description: "Capture flag, both fixture roots, and every fixture found under them.",
  },
  {
    name: "dev_shape_report",
    group: "Dev · Fixtures",
    dev: true,
    description:
      "Reads every captured payload back and reports what the parser did not understand \u2014 event names with no `classify_event` arm, and files that are not JSON at all. A report, not a validator: nothing here changes what the parser accepts. `HordeKill` sat in captures for months while Voidgrubs never became markers.",
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
