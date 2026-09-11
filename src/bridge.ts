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
  UpdateStatus,
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

// Commands that are *not* in the Rust dispatch table and so must be invoked
// directly. Everything else goes through the `rpc` passthrough, which is what
// lets the backend register one command instead of twenty-three and generate
// its name list instead of hand-writing it (DEVELOPMENT.md §12).
//
// Two reasons a command is on this list:
//   - it drives the desktop shell, so it belongs to the UI process and not
//     the recorder — `open_recordings_folder`, `dev_open_portal`;
//   - it is a `dev_*` command, which stays individually registered behind the
//     `devtools` Cargo feature. `dev_registered_commands` in particular *must*
//     stay direct: `devportal.ts` detects whether the portal exists by seeing
//     that call reject in a shipped build, and routing it through `rpc` would
//     make it reject with "unknown command" in *every* build — permanently
//     hiding the button.
//
// The portal's own panels use a separate invoke layer (`src/dev/ipc.ts`) and
// are unaffected.
const DIRECT_COMMANDS = new Set([
  "open_recordings_folder",
  "dev_open_portal",
  "dev_registered_commands",
]);

export async function call<T>(
  command: string,
  args?: Record<string, unknown>,
): Promise<T> {
  if (IN_TAURI) {
    if (DIRECT_COMMANDS.has(command)) return invoke<T>(command, args);
    // `args` is forwarded untouched, so the wire shape is unchanged: the
    // camelCase the callers below already send is what Rust now maps itself.
    return invoke<T>("rpc", { command, args: args ?? {} });
  }
  if (import.meta.env.DEV) return mock<T>(command, args);
  throw new Error(`invoke("${command}") outside Tauri`);
}

/**
 * Whether this build carries the `devtools` Cargo feature, decided the only
 * way the frontend can decide it: ask whether the `dev_*` commands are
 * registered. A shipped build rejects the call, and that rejection *is* the
 * answer, so this resolves `false` rather than throwing.
 *
 * Memoised because two callers want it — `devportal.ts` to reveal the portal
 * button, and `desktop.ts` to leave the webview's own context menu and
 * reload key alone in a build that can inspect — and one probe is enough.
 */
let devCommands: Promise<boolean> | null = null;

export function hasDevCommands(): Promise<boolean> {
  devCommands ??= call<string[]>("dev_registered_commands")
    .then((commands) => commands.length > 0)
    .catch(() => false);
  return devCommands;
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
    game_mode: "CLASSIC",
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
    // What the finalize observed, as against what the file holds. Nothing
    // in the main UI reads it — the dev portal does (#72) — but the mock
    // carries a realistic one so a browser-only session is not the odd
    // case out.
    // Ten champions, ours marked, so a browser-only session has a
    // scoreboard to draw before anyone has played a game — and so the
    // "no scoreboard" case is visibly a different row rather than the
    // only one that ever renders.
    scoreboard_json: JSON.stringify(fixtureScoreboard(champion)),
    cs: champion === null ? null : 180 + id * 11,
    // A ladder on most rows, and deliberately not on all of them: an
    // unranked game and a ranked one have to look different in a
    // browser-only session, or the empty case is the one nobody sees.
    tier: champion === null ? null : id % 3 === 0 ? "MASTER" : "EMERALD",
    division: champion === null || id % 3 === 0 ? null : "III",
    lp_after: champion === null ? null : id % 3 === 0 ? 412 : 38,
    diagnostics_json: JSON.stringify({
      game_id: 5000 + id,
      queue_id: 420,
      is_custom: false,
      polls: 1487,
      first_game_time_s: 0.4,
      last_game_time_s: 1495.2,
      ever_matched: true,
      alignment_offset_s: 18.6,
      backend: "stub",
      markers: 6,
      samples: 1486,
    }),
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
/**
 * A plausible ten-player scoreboard for the fixtures.
 *
 * `null` champion means the live poller never matched us, which is the
 * shape a rescan import has — no scoreboard at all, so the row has to
 * render without one.
 */
function fixtureScoreboard(champion: string | null) {
  if (champion === null) return null;
  const cast = ["Garen", "Blitzcrank", "Jinx", "Thresh", "Zed", "Lux", "Nautilus", "Akali", "Pantheon"];
  const items = [3089, 3157, 3020, 3135, 3116, 3363];
  return {
    our_team: "ORDER",
    our_runes: {
      keystone_id: 8112,
      keystone: "Electrocute",
      primary_tree_id: 8100,
      secondary_tree_id: 8300,
    },
    players: [champion, ...cast].map((name, i) => ({
      champion: name,
      team: i < 5 ? "ORDER" : "CHAOS",
      is_us: i === 0,
      level: 11 + (i % 5),
      kills: i === 0 ? 15 : (i * 2) % 9,
      deaths: i === 0 ? 3 : (i + 1) % 7,
      assists: i === 0 ? 5 : (i * 3) % 11,
      cs: 120 + i * 17,
      items: items.slice(0, 4 + (i % 3)),
      spells: ["Flash", i % 2 === 0 ? "Ignite" : "Teleport"],
    })),
  };
}

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
  // Live Client Data only: a Practice Tool game, or one recorded with no
  // LCU summary. There is no queue id to show, so the card has to fall
  // back to the mode rather than going blank.
  row(14, "Ahri", null, { queue: null, game_mode: "PRACTICETOOL", win: null }),
  // What `reconcile` produces for a file dropped into the folder: path and
  // size are all it knows.
  row(9, null, null, {
    champion: null,
    win: null,
    duration_s: null,
    queue: null,
    game_mode: null,
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

/**
 * How far into the video the game clock starts, in the fixtures.
 *
 * A real recording begins on the loading screen and carries about twenty
 * seconds of it before anything happens — which is exactly what the review
 * player's window has to skip, so a browser-only session has to have one to
 * skip. The old fixtures used five seconds, which is short enough that the
 * behaviour was invisible.
 */
const LOADING_SCREEN_S = 20;

const FIXTURE_MARKERS: MarkerRow[] = [
  ["first_blood", 132], ["kill", 240], ["death", 415], ["dragon", 602],
  ["assist", 745], ["kill", 760], ["multikill", 762], ["turret", 900],
  ["death", 1105], ["voidgrubs", 1150], ["herald", 1180], ["baron", 1420], ["ace", 1444],
  ["kill", 1460], ["inhibitor", 1600],
].map(([kind, t], i) => ({
  id: i + 1,
  recording_id: 1,
  game_time_s: t as number,
  video_time_s: (t as number) + LOADING_SCREEN_S,
  kind: kind as string,
  payload_json: "{}",
}));

// A plausible arc: even early, ahead mid, thrown late.
const curve = (t: number) => Math.sin(t / 200) * 4200 + t * 2.4 - 900;

/**
 * Two densities, because that is what the real table holds.
 *
 * Kill and CS diffs are sampled live at 1 Hz; gold comes from the post-game
 * match timeline at one frame a minute and lands as rows of its own with
 * every other metric NULL (`lcu::timeline`). A mock that put all three on
 * every row would hide exactly the case the renderer has to handle — and
 * hide the "no gold data" state entirely.
 */
const FIXTURE_SAMPLES: SampleRow[] = [
  ...Array.from({ length: 300 }, (_, i) => {
    const t = i * 5;
    return {
      id: i + 1,
      recording_id: 1,
      game_time_s: t,
      video_time_s: t + LOADING_SCREEN_S,
      our_team: "ORDER",
      gold_diff: null,
      kill_diff: Math.round(curve(t) / 900),
      cs_diff: Math.round(curve(t) / 260),
      our_gold: 300 + (i % 40) * 55,
      our_level: Math.min(18, 1 + Math.floor(i / 17)),
    };
  }),
  ...Array.from({ length: 25 }, (_, i) => {
    const t = i * 60;
    return {
      id: 1000 + i,
      recording_id: 1,
      game_time_s: t,
      video_time_s: t + LOADING_SCREEN_S,
      our_team: "ORDER",
      gold_diff: curve(t),
      kill_diff: null,
      cs_diff: null,
      our_gold: null,
      our_level: null,
    };
  }),
];

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
  // The interesting one to look at: an offer is the only state with a badge,
  // a button and a changelog. The three duller ones are a one-word edit away.
  //
  // Shaped exactly like what CI writes into `latest.json` — a heading the
  // renderer drops, one bullet per commit, and the trailing compare link —
  // so a browser-only session exercises the real parse rather than a
  // tidied-up version of it.
  get_update_status: {
    kind: "available",
    offer: {
      version: "0.9.0",
      notes: [
        "## What's changed",
        "",
        "- feat(review): draw the objective bounty window on the timeline",
        "- fix(lcu): show the Riot ID as the summoner name",
        "- fix(recorder): stop the worker leaking a handle per game",
        "",
        "**Full changelog**: https://github.com/NinjaGoldfinch/ninja-recorder/compare/v0.8.0...v0.9.0",
      ].join("\n"),
      pub_date: null,
    },
    installable: true,
    blockedReason: null,
  } satisfies UpdateStatus,
  check_for_update: null,
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
    // Rejects rather than no-ops: outside the webview there is nothing to
    // restart into, and a button that silently "worked" would be the one
    // piece of this flow a browser session could not tell apart from real.
    case "install_update":
      throw new Error("updates are not available in this build");
  }
  if (command in MOCKS) return MOCKS[command] as T;
  throw new Error(`No dev fixture for invoke("${command}")`);
}
