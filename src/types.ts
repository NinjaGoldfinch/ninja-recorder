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
  /// JSON-encoded `AudioLayout`, or null when the layout is unknown —
  /// every recording made before multi-track audio, and anything a rescan
  /// imported from a file we didn't record.
  audio_tracks_json: string | null;
  /// Live Client Data's `gameData.gameMode` — "CLASSIC", "ARAM",
  /// "PRACTICETOOL". Not a queue id: `queue` holds Riot's real one and
  /// only the LCU can fill it, so a row can have either, both or neither.
  game_mode: string | null;
  /// JSON-encoded `RecordingDiagnostics` — what the app *observed* while
  /// making this recording, as against what the recording contains: how
  /// many Live Client Data polls landed, whether we were ever found in
  /// `allPlayers`, the alignment the markers were mapped through, which
  /// capture backend was live.
  ///
  /// Null for every row predating it and anything a rescan imported.
  /// Nothing in the main UI reads this; the dev portal does (#72).
  diagnostics_json: string | null;
  /// JSON `Scoreboard` — all ten champions as the game ended. Null for a
  /// game whose poller never saw a player list, for anything a rescan
  /// imported, and for every row predating migration 9.
  scoreboard_json: string | null;
  /// Our own creep score. Also inside `scoreboard_json`; a column of its
  /// own because the row sorts on it and CS per minute wants it beside
  /// `duration_s`.
  cs: number | null;
}

/** One capturable audio source. Mirrors Rust's `AudioSourceKind`. */
export type AudioSourceKind =
  | { kind: "game" }
  | { kind: "desktop" }
  | { kind: "microphone"; device_id?: string }
  | { kind: "application"; exe: string };

/** One audio track in the mp4. Track 0 is always the combined mix. */
export interface AudioTrackSpec {
  label: string;
  /** Indices into the layout's `sources`. */
  sources: number[];
}

export interface AudioLayout {
  sources: AudioSourceKind[];
  tracks: AudioTrackSpec[];
}

/**
 * What the user picked in Settings. Mirrors Rust's `AudioPreset`, which is
 * serde-tagged on `preset` — the backend validates it, so this type only has
 * to describe the shape, not enforce it.
 */
export type AudioPreset =
  | { preset: "game" }
  | { preset: "game_mic"; mic_device_id?: string }
  | { preset: "game_mic_discord"; mic_device_id?: string }
  | { preset: "desktop" }
  | { preset: "custom"; sources: AudioSourceKind[]; tracks: AudioTrackSpec[] };

/** A preset the settings screen can offer as a single button. */
export type AudioPresetKey = "game" | "game_mic" | "game_mic_discord" | "desktop";

/**
 * Start-on-login, as the platform reports it. Mirrors Rust's
 * `core::AutostartStatus`.
 *
 * Deliberately not a `Prefs` entry: this one lives in the Windows registry,
 * which the user can edit from Task Manager's Startup tab, so it is read from
 * the backend every time the settings view loads rather than cached.
 * `supported` is false in a build with no autostart control, where the row
 * shows disabled instead of a checkbox that would lie.
 */
export interface AutostartStatus {
  enabled: boolean;
  supported: boolean;
}

export interface AudioInputDevice {
  id: string;
  name: string;
  is_default: boolean;
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
  gold_diff: number | null;
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
  // Seconds since capture began, straight from the supervisor's session —
  // not timed in the UI, which would restart from zero if the window were
  // opened part-way through a game.
  recording_elapsed_s: number | null;
}

/** Mirrors `live_client::Scoreboard`, stored as `recordings.scoreboard_json`. */
export interface Scoreboard {
  players: ScoreboardPlayer[];
  /** Absent when the live poller never matched us in `allPlayers`, in which
   *  case the row cannot say which half is ours and shows neither. */
  our_team?: string;
  /** Ours only — the live API gives a full rune page for the active player
   *  and a reduced one for everybody else. */
  our_runes?: ScoreboardRunes;
}

export interface ScoreboardPlayer {
  champion: string;
  /** "ORDER" or "CHAOS". */
  team: string;
  /** True for the row's owner. Decided in Rust, where "which of these ten
   *  is us" already has exactly one answer — see `find_us`. */
  is_us?: boolean;
  level: number;
  kills: number;
  deaths: number;
  assists: number;
  cs: number;
  /** Item ids in slot order. Data Dragon files item art under the id. */
  items: number[];
  /** Display names — `["Flash", "Smite"]`. Art is filed under a key
   *  ("SummonerFlash"), so something has to map between them, the way
   *  champion art already does. */
  spells: string[];
}

export interface ScoreboardRunes {
  keystone_id: number;
  keystone: string;
  primary_tree_id: number;
  secondary_tree_id: number;
}

export interface ReconcileReport {
  orphans_removed: number;
  imported: number;
}

/** Mirrors `backfill::BackfillReport`. Every count is reported, not just
 *  the successes: "nothing matched" is a different answer from "nothing to
 *  do", and it is the user's cue that their recordings predate what their
 *  client still remembers. */
export interface BackfillReport {
  scanned: number;
  games_considered: number;
  matched: number;
  patched: number;
  ambiguous: number;
  unmatched: number;
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
