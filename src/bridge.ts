import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import type {
  AudioInputDevice,
  AudioLayout,
  AudioPreset,
  LcuStatus,
  MarkerRow,
  RecordingRow,
  SampleRow,
  SupervisorStatus,
} from "./types";

// Outside the Tauri webview there is no `invoke`, so every command
// rejects and the whole UI renders as an error state. That makes the plain
// `vite` dev server — which reloads far faster than a Tauri rebuild —
// useless for exactly the layout and theming work it's best at. In DEV we
// answer from fixtures instead; the branch is dead code in a production
// build and tree-shakes out.
const IN_TAURI = "__TAURI_INTERNALS__" in window;

export function isMocked(): boolean {
  return !IN_TAURI && import.meta.env.DEV;
}

// `convertFileSrc` reads the Tauri internals object directly, so it throws
// outside the webview rather than returning something useless. In DEV hand
// back the bare path: the video won't load, which lands the player on its
// error overlay — itself a state worth being able to look at.
export function assetUrl(path: string): string {
  if (IN_TAURI) return convertFileSrc(path);
  return path;
}

export async function call<T>(
  command: string,
  args?: Record<string, unknown>,
): Promise<T> {
  if (IN_TAURI) return invoke<T>(command, args);
  if (import.meta.env.DEV) return mock<T>(command, args);
  throw new Error(`invoke("${command}") outside Tauri`);
}

function row(
  id: number,
  champion: string | null,
  win: boolean | null,
  overrides: Partial<RecordingRow> = {},
): RecordingRow {
  // Fixed epoch, one day apart, so the fixtures don't shuffle between
  // reloads while you're comparing two versions of a card.
  const day = 24 * 60 * 60 * 1000;
  return {
    id,
    path: `/fixtures/recording-${id}.mp4`,
    started_at: Date.parse("2025-08-01T19:00:00Z") + id * day,
    duration_s: 1500 + id * 97,
    game_id: 5000 + id,
    queue: 420,
    champion,
    role: "MIDDLE",
    win,
    kda_k: 7,
    kda_d: 3,
    kda_a: 11,
    patch: "15.17",
    pinned: false,
    size_bytes: 1_900_000_000,
    audio_tracks_json: JSON.stringify(LAYOUTS.game_mic),
    ...overrides,
  };
}

// The three shapes the review player has to render: a single track (no stem
// picker at all), a multi-track recording, and an unknown layout.
const LAYOUTS = {
  game: {
    sources: [{ kind: "game" }],
    tracks: [{ label: "Game", sources: [0] }],
  },
  game_mic: {
    sources: [{ kind: "game" }, { kind: "microphone" }],
    tracks: [
      { label: "Everything", sources: [0, 1] },
      { label: "Game", sources: [0] },
      { label: "Mic", sources: [1] },
    ],
  },
} satisfies Record<string, AudioLayout>;

// Deliberately awkward: nulls everywhere a rescan-imported file has them,
// a champion name long enough to wrap a card, and a filename that would
// break out of an attribute if it were interpolated unescaped.
const FIXTURE_ROWS: RecordingRow[] = [
  row(1, "Ahri", true, { pinned: true }),
  // Single track: the player must hide the stem picker entirely.
  row(13, "Jinx", true, { audio_tracks_json: JSON.stringify(LAYOUTS.game) }),
  row(2, "Lee Sin", false, { kda_k: 2, kda_d: 9, kda_a: 4 }),
  row(3, "Aurelion Sol", true, { duration_s: 3120 }),
  row(4, "Kai'Sa", false),
  row(5, "Nunu & Willump", true),
  row(6, "Renata Glasc", false, { queue: 440 }),
  row(7, "Yasuo", true, { kda_k: 18, kda_d: 4, kda_a: 6 }),
  row(8, "Gwen", null, { win: null, kda_k: null, kda_d: null, kda_a: null }),
  // What `reconcile` produces for a file dropped into the folder: path and
  // size are all it knows.
  row(9, null, null, {
    champion: null,
    win: null,
    duration_s: null,
    queue: null,
    kda_k: null,
    kda_d: null,
    kda_a: null,
    path: '/fixtures/clip " onerror="alert(1).mp4',
    size_bytes: 240_000_000,
    // A rescan knows nothing about a file's audio.
    audio_tracks_json: null,
  }),
  row(10, "Kled", true, { pinned: true, size_bytes: 3_400_000_000 }),
  row(11, "Zed", false),
  row(12, "Twisted Fate", true, { duration_s: 880 }),
];

const FIXTURE_MARKERS: MarkerRow[] = [
  ["first_blood", 132], ["kill", 240], ["death", 415], ["dragon", 602],
  ["assist", 745], ["kill", 760], ["turret", 900], ["death", 1105],
  ["herald", 1180], ["baron", 1420], ["ace", 1444], ["kill", 1460],
].map(([kind, t], i) => ({
  id: i + 1,
  recording_id: 1,
  game_time_s: t as number,
  video_time_s: (t as number) + 5,
  kind: kind as string,
  payload_json: "{}",
}));

const FIXTURE_SAMPLES: SampleRow[] = Array.from({ length: 300 }, (_, i) => {
  const t = i * 5;
  // A plausible curve: even early, ahead mid, thrown late.
  const gold = Math.sin(i / 40) * 4200 + i * 12 - 900;
  return {
    id: i + 1,
    recording_id: 1,
    game_time_s: t,
    video_time_s: t + 5,
    our_team: "ORDER",
    gold_diff_est: gold,
    kill_diff: Math.round(gold / 900),
    cs_diff: Math.round(gold / 260),
    our_gold: 300 + (i % 40) * 55,
    our_level: Math.min(18, 1 + Math.floor(i / 17)),
  };
});

const MOCKS: Record<string, unknown> = {
  get_recording_markers: FIXTURE_MARKERS,
  get_recording_samples: FIXTURE_SAMPLES,
  get_retention_policy: { max_total_bytes: 53_687_091_200, max_age_days: 30 },
  preview_retention_policy: { deleted: [], freed_bytes: 0 },
  set_retention_policy: { deleted: [], freed_bytes: 0 },
  get_ui_prefs: {},
  get_audio_preset: { preset: "game" } satisfies AudioPreset,
  set_audio_preset: null,
  list_audio_inputs: [
    { id: "mic-usb", name: "Blue Yeti", is_default: true },
    { id: "mic-webcam", name: "HD Webcam Microphone", is_default: false },
    { id: "mic-line", name: "Line In (Realtek(R) Audio)", is_default: false },
  ] satisfies AudioInputDevice[],
  get_recordings_dir: "/fixtures/recordings",
  rescan_recordings: { orphans_removed: 0, imported: 0 },
  lcu_status: {
    connected: true,
    phase: "None",
    summoner: "FixtureSummoner",
    error: null,
  } satisfies LcuStatus,
  game_state_status: {
    state: "ClientRunning",
    last_finalized: null,
    recording_elapsed_s: null,
  } satisfies SupervisorStatus,
};

// Writes mutate the fixture array rather than no-op'ing, so pin and delete
// behave the way they will in the real app — a two-step delete that never
// removes anything is not much of a test.
async function mock<T>(
  command: string,
  args?: Record<string, unknown>,
): Promise<T> {
  switch (command) {
    case "list_recordings":
      return FIXTURE_ROWS as T;
    case "get_disk_usage":
      return {
        total_bytes: FIXTURE_ROWS.reduce((a, r) => a + r.size_bytes, 0),
        recording_count: FIXTURE_ROWS.length,
        free_bytes: 214_000_000_000,
      } as T;
    case "set_pinned": {
      const row = FIXTURE_ROWS.find((r) => r.id === args?.recordingId);
      if (row) row.pinned = Boolean(args?.pinned);
      return undefined as T;
    }
    case "delete_recording": {
      const at = FIXTURE_ROWS.findIndex((r) => r.id === args?.recordingId);
      if (at >= 0) FIXTURE_ROWS.splice(at, 1);
      return undefined as T;
    }
    case "set_ui_pref":
    case "open_recordings_folder":
      return undefined as T;
  }
  if (command in MOCKS) return MOCKS[command] as T;
  throw new Error(`No dev fixture for invoke("${command}")`);
}
