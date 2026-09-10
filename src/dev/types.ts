/**
 * TypeScript mirrors of the Rust structs the portal reads.
 *
 * Hand-written, like every other type in this project — see
 * `registry.ts` for why, and for the drift check that partially
 * compensates. Field names stay snake_case because serde does not rename
 * them on the way out; only *argument* names are camelCased by Tauri on
 * the way in.
 */

export type GameState = "Idle" | "ClientRunning" | "WaitingForGame" | "Recording" | "Finalizing";

/** The order of `DEVELOPMENT.md` §3.4's diagram, for the Overview readout. */
export const GAME_STATES: GameState[] = [
  "Idle",
  "ClientRunning",
  "WaitingForGame",
  "Recording",
  "Finalizing",
];

export interface SessionMarker {
  kind: string;
  game_time_s: number;
  payload: unknown;
  video_time_s: number;
}

export interface FinalizedRecording {
  recording_id: number | null;
  path: string;
  markers: SessionMarker[];
}

export interface SupervisorStatus {
  state: GameState;
  last_finalized: FinalizedRecording | null;
}

export interface DevSessionView {
  marker_count: number;
  sample_count: number;
  alignment_offset_s: number | null;
  elapsed_s: number;
  started_at_millis: number;
  recent_markers: SessionMarker[];
  last_sample: unknown;
}

export interface RetentionPolicy {
  max_total_bytes: number | null;
  max_age_days: number | null;
}

export interface DevHealth {
  supervisor: SupervisorStatus;
  session: DevSessionView | null;
  is_recording: boolean;
  total_bytes: number;
  free_bytes: number;
  counts: { recordings: number; markers: number; samples: number };
  policy: RetentionPolicy;
  replay_running: boolean;
  fixture_recording: boolean;
}

export interface DevEnvInfo {
  app_version: string;
  identifier: string;
  os: string;
  arch: string;
  build_profile: string;
  tauri_version: string;
  recorder_backend: string;
  app_data_dir: string;
  recordings_dir: string;
  db_path: string;
  fixtures_dir: string | null;
  repo_fixtures_dir: string | null;
  sample_mp4_present: boolean;
  lockfile_override: string | null;
  fixture_recording: boolean;
}

export interface LcuStatus {
  connected: boolean;
  phase: string | null;
  summoner: string | null;
  error: string | null;
}

export interface QueryResult {
  columns: string[];
  rows: unknown[][];
  rows_affected: number;
  elapsed_ms: number;
  returned_rows: boolean;
}

export interface TableSchema {
  name: string;
  row_count: number;
  columns: Array<{
    name: string;
    decl_type: string;
    not_null: boolean;
    default_value: string | null;
    pk: boolean;
  }>;
}

export interface SeedReport {
  recording_ids: number[];
  markers_inserted: number;
  samples_inserted: number;
  bytes_written: number;
  used_sample_mp4: boolean;
  paths: string[];
}

export interface PreviewRow {
  id: number;
  path: string;
  started_at: number;
  size_bytes: number;
  pinned: boolean;
  champion: string | null;
  file_exists: boolean;
}

export interface RetentionPreview {
  policy: RetentionPolicy;
  now_millis: number;
  total_bytes: number;
  pinned_bytes: number;
  to_delete: PreviewRow[];
  would_free_bytes: number;
  total_after_bytes: number;
}

export interface DispatchReport {
  before: SupervisorStatus;
  after: SupervisorStatus;
  session: DevSessionView | null;
}

export interface InjectReport {
  accepted: boolean;
  note: string | null;
  markers_added: number;
  samples_added: number;
  session: DevSessionView | null;
  state: string;
}

export interface ReplayStatus {
  running: boolean;
  game_time_s: number;
  duration_s: number;
  ticks: number;
  events_fired: number;
  finished: boolean;
  error: string | null;
}

export interface FixtureEntry {
  group: string;
  name: string;
  path: string;
  bytes: number;
  modified_millis: number | null;
  source: "captured" | "repo";
}

export interface FixturesState {
  recording_enabled: boolean;
  capture_dir: string | null;
  repo_dir: string | null;
  entries: FixtureEntry[];
}

export interface RecordingRow {
  id: number;
  path: string;
  started_at: number;
  duration_s: number | null;
  game_id: number | null;
  queue: number | null;
  champion: string | null;
  role: string | null;
  win: boolean | null;
  kda_k: number | null;
  kda_d: number | null;
  kda_a: number | null;
  patch: string | null;
  pinned: boolean;
  size_bytes: number;
  /** JSON `RecordingDiagnostics` — see `RecordingDiagnostics` below. */
  diagnostics_json: string | null;
}

/** Mirrors Rust's `log::ParsedLine`. `level`/`tag` are empty for a line
 *  that does not fit the format — a panic backtrace, say — which is kept
 *  rather than dropped. */
export interface LogLine {
  timestamp: string;
  level: string;
  tag: string;
  message: string;
}

/** Mirrors Rust's `dev::log_api::LogPage`. */
export interface LogPage {
  file: string;
  dir: string | null;
  /** Lines in the file, before filtering. */
  total: number;
  /** Lines that survived the filters, which may exceed those returned. */
  matched: number;
  truncated: boolean;
  /** Oldest first, so the newest reads at the bottom. */
  lines: LogLine[];
  tags_present: string[];
}

/** Mirrors Rust's `dev::log_api::LogFileInfo`. */
export interface LogFileInfo {
  name: string;
  bytes: number;
  modified_millis: number | null;
  active: boolean;
  exists: boolean;
}

/** Mirrors Rust's `state_machine::supervisor::RecordingDiagnostics` —
 *  what the app observed while making a recording, as against what the
 *  recording contains. Parsed out of `RecordingRow.diagnostics_json`. */
export interface RecordingDiagnostics {
  game_id: number | null;
  queue_id: number | null;
  is_custom: boolean;
  polls: number;
  first_game_time_s: number | null;
  last_game_time_s: number | null;
  ever_matched: boolean;
  alignment_offset_s: number | null;
  backend: string;
  markers: number;
  samples: number;
}

/** Mirrors `dev::recording_api::Provenance`. `source` is null when nothing
 *  wrote the field, which is its own answer — the panel must not invent a
 *  plausible-looking writer for an empty value. */
export interface Provenance {
  field: string;
  source: string | null;
  note: string;
}

/** Mirrors `dev::recording_api::RecordingReport` — one recording from every
 *  source that knows something about it (#99). `gold_samples` is counted
 *  apart from `samples` because they come from different sources on
 *  different schedules; plenty of samples and no gold is the #137
 *  fingerprint rather than a contradiction. */
/** How a stored value and the client's current answer relate. */
export type Verdict = "agree" | "differ" | "only_stored" | "only_live" | "neither";

export interface FieldComparison {
  field: string;
  stored: string | null;
  live: string | null;
  verdict: Verdict;
}

export interface LcuComparison {
  game_id: number;
  fields: FieldComparison[];
  differing: number;
}

// One definition, shared with the main window's Settings view — the two
// read the same command and must not drift apart.
export type { BackfillReport } from "../types";

export interface RecordingReport {
  row: RecordingRow;
  provenance: Provenance[];
  markers: number;
  samples: number;
  gold_samples: number;
  alignment_offset_s: number | null;
  scoreboard: unknown | null;
  diagnostics: unknown | null;
}
