// Shapes mirroring the Rust serde structs. They live here rather than
// beside their first consumer so the modules that need them don't have to
// import each other — `bridge`'s dev fixtures need the row type, and
// `review` needs `bridge`, which would otherwise be a cycle.

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
}

export interface MarkerRow {
  id: number;
  recording_id: number;
  game_time_s: number;
  video_time_s: number;
  kind: string;
  payload_json: string;
}

export interface SampleRow {
  id: number;
  recording_id: number;
  game_time_s: number;
  video_time_s: number;
  our_team: string | null;
  gold_diff_est: number | null;
  kill_diff: number | null;
  cs_diff: number | null;
  our_gold: number | null;
  our_level: number | null;
}

export interface LcuStatus {
  connected: boolean;
  phase: string | null;
  summoner: string | null;
  error: string | null;
}

export interface SessionMarker {
  kind: string;
  game_time_s: number;
  video_time_s: number;
  payload: unknown;
}

export interface FinalizedRecording {
  recording_id: number | null;
  path: string;
  markers: SessionMarker[];
}

export type GameState =
  | "Idle"
  | "ClientRunning"
  | "WaitingForGame"
  | "Recording"
  | "Finalizing";

export interface SupervisorStatus {
  state: GameState;
  last_finalized: FinalizedRecording | null;
}

export interface ReconcileReport {
  orphans_removed: number;
  imported: number;
}

export interface DiskUsage {
  total_bytes: number;
  recording_count: number;
  free_bytes: number;
}

export interface RetentionPolicy {
  max_total_bytes: number | null;
  max_age_days: number | null;
}

// What a retention sweep did — or, from `preview_retention_policy`, what
// it would do if the policy were saved.
export interface EnforcementReport {
  deleted: number[];
  freed_bytes: number;
}
