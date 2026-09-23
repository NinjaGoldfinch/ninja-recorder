// GENERATED FILE. Do not edit by hand.
//
// Regenerate with:  cargo run --features contract-gen --bin gen-contract   (from src-tauri/)
// CI runs the same binary with --check and fails if this file is stale.
//
// The declaration lives in Rust:
//   commands  src-tauri/src/core/dispatch.rs   (dispatch_table!)
//   events    src-tauri/src/contract/events.rs (contract_events!)
//   types     src-tauri/src/contract/types.rs  (the boundary list)

export type BackfillReport = { 
/**
 * Rows that were missing metadata when the pass started.
 */
scanned: number, 
/**
 * Games the client offered.
 */
games_considered: number, 
/**
 * Rows resolved to exactly one game: by the `game_id` the row already
 * carried where it had one, and by the clock otherwise.
 */
matched: number, 
/**
 * Rows actually written. Lower than `matched` when a row was deleted
 * mid-pass, or when the matched game said nothing worth writing.
 */
patched: number, 
/**
 * Rows where more than one game overlapped, so nothing was written.
 */
ambiguous: number, 
/**
 * Rows no game overlapped.
 */
unmatched: number, 
/**
 * Rows that regained a gold curve. Separate from `patched` because the
 * two fail independently: the timeline is a different endpoint, and a
 * game old enough to have fallen out of match history can still yield
 * metadata while having no timeline left to fetch.
 */
gold_filled: number, 
/**
 * Rows that gained a scoreboard they did not have — every recording
 * made before the live capture existed.
 */
scoreboards: number, };

export type Event = { "type": "stateChanged", state: GameState, 
/**
 * Epoch milliseconds at which this state was entered, so a client can
 * render "recording for 4:12" without timing it itself and without
 * drifting when the window was asleep.
 */
sinceMs: number, } | { "type": "lcuPhase", phase: GameflowPhase | null, clientPresent: boolean, } | { "type": "recordingStarted", 
/**
 * `None` until finalize: the library row is written when the recording
 * ends, so nothing has an id while it is still being captured. A
 * client correlates on `file_stem` until `RecordingStopped` names the
 * row.
 */
recordingId: number | null, 
/**
 * **The stem, not the path.** `Recorder::start` returns `Ok(())`; the
 * file it actually wrote — extension and all, which depends on the
 * backend — is only known when `stop` hands back a `RecordingOutput`.
 * The stem is what capture was asked for, it is unique per recording
 * (it carries `started_at_millis`), and it is a substring of the path
 * the library row ends up with. Publishing a guessed path instead
 * would be a value that is sometimes wrong.
 */
fileStem: string, startedAtMs: number, } | { "type": "recordingStopped", recordingId: number | null, outcome: StopOutcome, } | { "type": "markerAdded", recordingId: number | null, marker: SessionMarker, } | { "type": "sampleBatch", recordingId: number | null, samples: Array<SessionSample>, } | { "type": "matchSummaryPatched", recordingId: number, } | { "type": "libraryChanged", reason: LibraryChangeReason, } | { "type": "retentionRan", deleted: Array<number>, freedBytes: number, } | { "type": "updateStatus", status: UpdateStatus, } | { "type": "daemonShuttingDown", reason: ShutdownReason, } | { "type": "lagged", dropped: number, } | { "type": "showUi", 
/**
 * The view to land on, in `router.ts`'s vocabulary, or `None` for
 * wherever the window was. `Some("settings")` is the Settings item.
 */
view: string | null, };

export type LibraryChangeReason = "finalized" | "edited" | "reconciled" | "retention";

export type ShutdownReason = "quit" | "update" | "error";

export type StopOutcome = { "kind": "clean" } | { "kind": "crashed" } | { "kind": "refused", reason: string, };

export type Topic = "recording" | "lcu" | "library" | "update" | "daemon";

export type CurrentRecording = { 
/**
 * The file stem capture is writing to. A stem rather than a path because
 * `Recorder::start` returns `Ok(())` and the extension is not settled
 * until `stop` hands back a `RecordingOutput`.
 */
fileStem: string, 
/**
 * When capture actually started, in epoch milliseconds.
 */
startedAtMillis: number, 
/**
 * Seconds of capture so far. Read from the session rather than timed by
 * the client, so a window opened part-way through a game does not report
 * a counter that started when it happened to look.
 */
elapsedS: number, 
/**
 * Markers collected so far this game.
 */
markerCount: number, 
/**
 * Advantage samples collected so far this game.
 */
sampleCount: number, 
/**
 * The game-time to video-time offset in force, or `None` while the game
 * clock has not been seen to advance.
 *
 * `None` is not zero. "Not yet known" and "aligned" are different states,
 * and reading one as the other is how a marker lands in the wrong place
 * ([docs/recording-pipeline.md](../../../docs/recording-pipeline.md)).
 */
alignmentOffsetS: number | null, };

export type Snapshot = { 
/**
 * The position in the event stream this snapshot was taken at. See the
 * module header: events at or below it are already included here.
 */
seq: number, 
/**
 * What the state machine is doing.
 *
 * Deliberately the bare `GameState` and not `SupervisorStatus`. That type
 * also carries `last_finalized`, which is a whole `FinalizedRecording`
 * including every marker of the last game. A snapshot is sent on every
 * connect and reconnect, so a field that grows with the length of a game
 * is the wrong shape for it, and the library already answers "what did the
 * last game produce" through `list_recordings` and
 * `get_recording_markers`. What is live rather than stored is in
 * `current_recording` below.
 */
state: GameState, 
/**
 * Whether the League client is there, and what it says it is doing.
 */
lcu: LcuStatus, 
/**
 * The recording in flight, when there is one. `None` is the resting
 * state and is not an error.
 */
currentRecording: CurrentRecording | null, 
/**
 * The last thing the background update check found.
 */
update: UpdateStatus, 
/**
 * The `settings_kv` rows the frontend treats as its preference cache.
 *
 * A map rather than a struct because the table is deliberately
 * schemaless: a missing key means "use the frontend default", which is
 * what lets a new preference ship without a migration
 * ([DEVELOPMENT.md §5.1](../../../DEVELOPMENT.md)).
 */
prefs: { [key in string]: string }, };

export type AutostartStatus = { 
/**
 * The platform's answer, re-read after any change — never what was
 * asked for.
 */
enabled: boolean, 
/**
 * False when this build has no autostart control at all, which is the
 * signal to show the row disabled rather than a checkbox that lies.
 */
supported: boolean, };

export type DiskUsage = { total_bytes: number, recording_count: number, free_bytes: number, };

export type LcuStatus = { connected: boolean, phase: string | null, summoner: string | null, error: string | null, };

export type QuitOutcome = { "outcome": "shuttingDown" } | { "outcome": "recordingInFlight" };

export type MarkerRow = { id: number, recording_id: number, game_time_s: number, video_time_s: number, kind: string, payload_json: string, };

export type RecordingRow = { id: number, path: string, started_at: number, duration_s: number | null, game_id: number | null, queue: number | null, champion: string | null, role: string | null, win: boolean | null, kda_k: number | null, kda_d: number | null, kda_a: number | null, patch: string | null, pinned: boolean, size_bytes: number, 
/**
 * JSON `recorder::audio::AudioLayout`. `None` = unknown, which is the
 * right answer for a file we did not record.
 */
audio_tracks_json: string | null, 
/**
 * Live Client Data's `gameMode`. The library card falls back to this
 * for its Queue label when `queue` is NULL — see migration 6.
 */
game_mode: string | null, 
/**
 * JSON `state_machine::supervisor::RecordingDiagnostics`. `None` for anything
 * recorded before migration 7 and anything `reconcile` imported.
 */
diagnostics_json: string | null, 
/**
 * JSON `live_client::Scoreboard`. `None` for a game whose poller
 * never saw a player list, and for anything `reconcile` imported.
 */
scoreboard_json: string | null, 
/**
 * Our own creep score. Also inside `scoreboard_json`; here as well
 * because the row sorts on it.
 */
cs: number | null, 
/**
 * The ladder this game was played at (#149). `None` on everything that
 * is not a ranked game, everything recorded before migration 10, and
 * any patch that landed too late to be sure the rank still described
 * this game — see `match_summary::RANK_FRESHNESS`.
 */
tier: string | null, 
/**
 * `None` at Master and above, where divisions do not exist.
 */
division: string | null, 
/**
 * LP once the game settled.
 */
lp_after: number | null, 
/**
 * LP when the game started. The other end of the measurement.
 */
lp_before: number | null, 
/**
 * What the game moved, measured across those two readings (#164).
 * `None` wherever a number would have been wrong rather than merely
 * unknown — see `lcu::ranked::lp_delta`.
 */
lp_delta: number | null, };

export type RetentionPolicy = { max_total_bytes: number | null, max_age_days: number | null, };

export type SampleRow = { id: number, recording_id: number, game_time_s: number, video_time_s: number, our_team: string | null, gold_diff: number | null, kill_diff: number | null, cs_diff: number | null, our_gold: number | null, our_level: number | null, };

export type ReconcileReport = { orphans_removed: number, imported: number, };

export type IconRequest = { champions: Array<string>, items: Array<number>, spells: Array<string>, 
/**
 * The same spells as ids, for a scoreboard rebuilt from match
 * history — it reports ids where the live client reports names.
 */
spellIds: Array<number>, runes: Array<number>, };

export type IconSet = { champions: { [key in string]: string }, items: { [key in string]: string }, spells: { [key in string]: string }, spell_ids: { [key in string]: string }, runes: { [key in string]: string }, };

export type GameflowPhase = "None" | "Lobby" | "Matchmaking" | "CheckedIntoTournament" | "ReadyCheck" | "ChampSelect" | "GameStart" | "FailedToLaunch" | "InProgress" | "Reconnect" | "WaitingForStats" | "PreEndOfGame" | "EndOfGame" | "TerminatedInError" | { "Unknown": string };

export type Marker = { kind: MarkerKind, game_time_s: number, 
/**
 * Structured detail specific to the marker kind (killer/victim/dragon
 * type/etc.) — matches the `payload_json` column planned in
 * DEVELOPMENT.md §4, so this serializes straight into the DB.
 *
 * `unknown` rather than `any` on the TypeScript side, and deliberately:
 * this really is an untyped blob today, and `unknown` forces a consumer
 * to narrow it before use where `any` would let a typo through silently.
 *
 * The shape it *should* have is a discriminated union keyed on `kind` —
 * `Kill { killer, victim }`, `Dragon { kind }`, and so on. That is a real
 * change to this type rather than an annotation, so it is not WS2.2's.
 */
payload: unknown, };

export type MarkerKind = "kill" | "death" | "assist" | "dragon" | "baron" | "herald" | "voidgrubs" | "turret" | "inhibitor" | "ace" | "multikill" | "first_blood";

export type Scoreboard = { 
/**
 * Both teams, in the order the response listed them.
 */
players: Array<ScoreboardPlayer>, 
/**
 * `"ORDER"` or `"CHAOS"`, or absent when we could not be matched —
 * in which case the row cannot say which half is ours and shows
 * neither.
 */
our_team: string | null, 
/**
 * Ours only: the live API gives the full rune page for the active
 * player and a reduced one for everybody else.
 */
our_runes: ScoreboardRunes | null, };

export type ScoreboardPlayer = { champion: string, 
/**
 * `"ORDER"` or `"CHAOS"`.
 */
team: string, 
/**
 * True for the row's owner, so the frontend does not have to match
 * names a second time — that question has one answer here (`find_us`)
 * and it should not grow a second one in TypeScript.
 */
is_us?: boolean, level: number, kills: number, deaths: number, assists: number, cs: number, 
/**
 * `Top` / `Jungle` / `Middle` / `Bottom` / `Support`, in the same words
 * the `role` column uses.
 *
 * Carried for every player, not just ours, because it is the only thing
 * that can say which of the five opponents was *the* opponent — the row
 * draws a matchup, and a lane opponent picked by list order would be a
 * guess dressed as a fact. `None` where the game said nothing, which is
 * every recording made before this field existed and any mode with no
 * positions to assign.
 */
position?: string | null, 
/**
 * Item ids in slot order, trinket included. Empty slots are dropped
 * rather than zero-filled: a zero is an item id that does not exist,
 * and the row draws as many boxes as it wants regardless.
 */
items: Array<number>, 
/**
 * Spell display names — `["Flash", "Smite"]`. Names rather than the
 * keys Data Dragon files art under, because the name is the half that
 * survives a response shape changing; mapping one to the other is the
 * art layer's job, as it already is for champions.
 */
spells: Array<string>, 
/**
 * The same two spells as ids, which is all match history gives.
 *
 * A scoreboard captured live has names and no ids; one rebuilt from
 * match history has ids and no names. Both are enough to find the
 * art, so both are stored rather than one being converted into the
 * other — converting would need the CDN, in a path that otherwise
 * only talks to the League client.
 */
spell_ids?: Array<number>, };

export type ScoreboardRunes = { keystone_id: number, keystone: string, primary_tree_id: number, secondary_tree_id: number, };

export type TeamDiff = { 
/**
 * "ORDER" or "CHAOS" — which side we were on. Persisted alongside the
 * diffs so the sign convention stays auditable after the fact.
 */
our_team: string, 
/**
 * **Estimated.** The Live Client Data API exposes no per-player gold
 * at all — `activePlayer.currentGold` is our own *unspent* gold and is
 * the only gold field in the entire response. This approximates each
 * team's earned gold as the summed price of the items its players are
 * currently holding, plus our unspent gold on our side only. It drifts
 * from true gold via sold items, consumed consumables, component-vs-
 * completed-item pricing, and the enemy's unknowable unspent gold, so
 * it must never be presented to the user as an exact figure.
 * Exact, from `allPlayers[].scores`.
 */
kill_diff: number, 
/**
 * Exact, from `allPlayers[].scores`.
 */
cs_diff: number, };

export type AudioInputDevice = { 
/**
 * The endpoint id the capture backend wants, verbatim.
 */
id: string, name: string, is_default: boolean, };

export type AudioLayout = { sources: Array<AudioSourceKind>, tracks: Array<AudioTrackSpec>, };

export type AudioPreset = { "preset": "game" } | { "preset": "game_mic", mic_device_id?: string | null, } | { "preset": "game_mic_discord", mic_device_id?: string | null, } | { "preset": "desktop" } | { "preset": "custom", sources: Array<AudioSourceKind>, tracks: Array<AudioTrackSpec>, } | { "preset": "unknown" };

export type AudioSourceKind = { "kind": "game" } | { "kind": "desktop" } | { "kind": "microphone", device_id?: string | null, } | { "kind": "application", exe: string, };

export type AudioTrackSpec = { label: string, sources: Array<number>, };

export type CaptureBackend = "libobs" | "own";

export type CaptureBackendOption = { backend: CaptureBackend, 
/**
 * Why it cannot be built here, or `null` when it can. A reason rather
 * than a flag, because the settings row shows it beside the disabled
 * choice and a bare "unavailable" gives nobody anything to act on.
 */
unavailable: string | null, };

export type CaptureBackendStatus = { 
/**
 * The saved choice, or the default if none was ever saved.
 */
configured: CaptureBackend, 
/**
 * What the live backend says it is (`Recorder::backend_name`), e.g.
 * `libobs (ready)` or `unavailable (…)`. The two differ when the
 * configured backend was refused, and this is how the row finds out.
 */
active: string, 
/**
 * Every backend this build knows about, available or not, in the order
 * the control lists them.
 */
options: Array<CaptureBackendOption>, };

export type EnforcementReport = { deleted: Array<number>, freed_bytes: number, };

export type GameState = "Idle" | "ClientRunning" | "WaitingForGame" | "Recording" | "Finalizing";

export type FinalizedRecording = { recording_id: number | null, path: string, markers: Array<SessionMarker>, };

export type RecordingDiagnostics = { 
/**
 * What the gameflow session said, and `game_id: None` is itself the
 * answer to "why is queue NULL".
 */
game_id: number | null, queue_id: number | null, is_custom: boolean, 
/**
 * Successful Live Client Data polls. Not the same as `samples`, which
 * only grow when the game clock advances — a gap between the two is a
 * loading screen, a pause, or a stalled clock.
 */
polls: number, first_game_time_s: number | null, last_game_time_s: number | null, 
/**
 * Whether we were ever found in `allPlayers`. `false` is the whole
 * explanation for a NULL champion, a NULL KDA and an empty advantage
 * curve, and it is otherwise invisible — see `find_us`.
 */
ever_matched: boolean, 
/**
 * The game-time-to-video-time offset in force at the end, or `None`
 * if the clock was never seen to advance. A marker that seeks to the
 * wrong moment is this number being wrong.
 */
alignment_offset_s: number | null, 
/**
 * Which capture backend was actually live, which on Windows may be
 * `FailedRecorder` carrying its init error.
 */
backend: string, 
/**
 * What the finalize wrote. A count here that disagrees with the
 * `markers`/`samples` tables means an insert failed.
 */
markers: number, samples: number, };

export type SessionMarker = { video_time_s: number, kind: MarkerKind, game_time_s: number, 
/**
 * Structured detail specific to the marker kind (killer/victim/dragon
 * type/etc.) — matches the `payload_json` column planned in
 * DEVELOPMENT.md §4, so this serializes straight into the DB.
 *
 * `unknown` rather than `any` on the TypeScript side, and deliberately:
 * this really is an untyped blob today, and `unknown` forces a consumer
 * to narrow it before use where `any` would let a typo through silently.
 *
 * The shape it *should* have is a discriminated union keyed on `kind` —
 * `Kill { killer, victim }`, `Dragon { kind }`, and so on. That is a real
 * change to this type rather than an annotation, so it is not WS2.2's.
 */
payload: unknown, };

export type SessionSample = { game_time_s: number, video_time_s: number, 
/**
 * `None` when the active player couldn't be matched in `allPlayers` —
 * see `live_client::events::team_diff` for why that isn't guessed.
 */
diff: TeamDiff | null, our_gold: number, our_level: number, };

export type SupervisorStatus = { state: GameState, last_finalized: FinalizedRecording | null, 
/**
 * Seconds since capture actually started, or `None` when nothing is
 * recording. Read from the session rather than timed by the UI: the
 * window can be opened part-way through a game, and a counter that
 * starts at zero when the UI first looks would misreport how much has
 * been captured.
 */
recording_elapsed_s: number | null, };

export type UpdateOffer = { version: string, 
/**
 * The release notes, as `latest.json` carried them. Shown verbatim and
 * therefore escaped by the caller — this is remote text.
 */
notes: string | null, pub_date: string | null, };

export type UpdateStatus = { "kind": "unsupported" } | { "kind": "checking" } | { "kind": "upToDate" } | { "kind": "available", offer: UpdateOffer, 
/**
 * Whether clicking Install right now is safe. Recomputed on every
 * read rather than stored, so a status first built mid-game does not
 * stay blocked after the game ends.
 */
installable: boolean, 
/**
 * Why not, when `installable` is false. Rendered next to the
 * disabled button — a greyed-out control with no reason is the
 * version of this that generates bug reports.
 */
blockedReason: string | null, } | { "kind": "failed", error: string, };

