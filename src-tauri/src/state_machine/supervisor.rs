//! Async orchestration around the pure `StateMachine`: spawns/aborts the
//! lockfile, gameflow, and Live Client Data watchers per `Action`, and
//! drives the `Recorder` and the marker pipeline (`live_client`).
//! DEVELOPMENT.md §3.4.
//!
//! Unlike `machine.rs`, most of this glue can't be meaningfully unit-tested
//! without a real LCU/Live Client Data connection — no League client is
//! installed on the machine this was written on. It's kept as thin as
//! possible over the well-tested pure transition function specifically so
//! the untested surface is small: `execute` mostly just spawns/aborts
//! tasks and calls the already-tested `Recorder` trait methods.
//!
//! The exceptions are the two pieces that hold real logic, and both are
//! shaped so they can be driven directly: `start_recording`/`stop_recording`
//! (stub `Recorder` + in-memory DB) and `RecordingSession::ingest`, which
//! takes elapsed time as an argument rather than reading the clock so a
//! whole game's poll sequence can be replayed in a test.

use crate::{debug, error, info, warn};
use super::machine::{Action, GameState, StateEvent, StateMachine};
use crate::db::{self, Db};
use crate::lcu;
use crate::live_client::{
    self, AlignmentTracker, AllGameData, LiveSummary, Marker, MarkerTracker, TimeAlignment,
};
use crate::live_client::team_diff;
use crate::recorder::{RecordConfig, Recorder};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::async_runtime::JoinHandle;

// `Deserialize` too: WS3.4 reads these back off the pipe in the UI process.
#[derive(Debug, Clone, Serialize, ts_rs::TS, serde::Deserialize)]
pub struct SessionMarker {
    #[serde(flatten)]
    pub marker: Marker,
    pub video_time_s: f64,
}

/// One 1 Hz sample of the team-advantage series, time-aligned to the video
/// the same way markers are. Kept in memory for the duration of the
/// recording and flushed to the DB in one transaction at finalize.
// `Deserialize` too: WS3.4 reads these back off the pipe in the UI process.
#[derive(Debug, Clone, Serialize, ts_rs::TS, serde::Deserialize)]
pub struct SessionSample {
    pub game_time_s: f64,
    pub video_time_s: f64,
    /// `None` when the active player couldn't be matched in `allPlayers` —
    /// see `live_client::events::team_diff` for why that isn't guessed.
    pub diff: Option<live_client::TeamDiff>,
    pub our_gold: f64,
    pub our_level: i64,
}

/// A marker as observed, before it has a position in the video.
///
/// `video_time_s` is deliberately *not* computed at ingest. The offset
/// between game time and video time is not knowable on the first poll (the
/// loading screen's clock is frozen) and does not stay constant afterwards
/// (pauses, dropped frames) — see `AlignmentTracker`. So each marker carries
/// the alignment that was in force when it arrived, and the mapping happens
/// once, at finalize. A marker seen before the clock ever moved has `None`
/// here and falls back at that point; it is never dropped.
#[derive(Debug, Clone)]
struct PendingMarker {
    marker: Marker,
    alignment: Option<TimeAlignment>,
}

impl PendingMarker {
    fn resolve(&self, fallback: TimeAlignment) -> SessionMarker {
        let alignment = self.alignment.unwrap_or(fallback);
        SessionMarker {
            video_time_s: alignment.video_time_s(self.marker.game_time_s),
            marker: self.marker.clone(),
        }
    }
}

/// One resolved marker as the `markers` table stores it.
///
/// A free function with two callers, which is the reason it exists: markers
/// are written twice now, once as they arrive during the game and once at
/// finalize against the final alignment (#150). Two copies of this mapping
/// would be two places for a column to be forgotten.
fn marker_row(m: &SessionMarker) -> db::NewMarker {
    db::NewMarker {
        game_time_s: m.marker.game_time_s,
        video_time_s: m.video_time_s,
        kind: m.marker.kind.as_str().to_string(),
        payload_json: m.marker.payload.to_string(),
    }
}

fn sample_row(s: &SessionSample) -> db::NewSample {
    db::NewSample {
        game_time_s: s.game_time_s,
        video_time_s: s.video_time_s,
        our_team: s.diff.as_ref().map(|d| d.our_team.clone()),
        // Left NULL on purpose. Gold is Riot's number now and arrives with
        // the summary patch, as its own sparser rows: nothing during the game
        // knows it (`lcu::timeline`).
        gold_diff: None,
        kill_diff: s.diff.as_ref().map(|d| d.kill_diff),
        cs_diff: s.diff.as_ref().map(|d| d.cs_diff),
        our_gold: Some(s.our_gold),
        our_level: Some(s.our_level),
    }
}

/// An advantage-curve sample before its video position is known. See
/// `PendingMarker` for why the mapping is deferred.
#[derive(Debug, Clone)]
struct PendingSample {
    game_time_s: f64,
    diff: Option<live_client::TeamDiff>,
    our_gold: f64,
    our_level: i64,
    alignment: Option<TimeAlignment>,
}

impl PendingSample {
    fn resolve(&self, fallback: TimeAlignment) -> SessionSample {
        let alignment = self.alignment.unwrap_or(fallback);
        SessionSample {
            game_time_s: self.game_time_s,
            video_time_s: alignment.video_time_s(self.game_time_s),
            diff: self.diff.clone(),
            our_gold: self.our_gold,
            our_level: self.our_level,
        }
    }
}

/// The one seam between the supervisor and whatever is listening. Named so
/// the `Mutex<Option<Box<dyn ...>>>` stack stays readable, and so the process
/// split has one type to change.
type EventNotifier = Box<dyn Fn(SupervisorEvent) + Send + Sync>;

/// Where contract events go — WS2.3's `EventSink`.
///
/// Separate from `EventNotifier` on purpose, and the two are not redundant.
/// `SupervisorEvent` is *internal*: it triggers desktop notifications and the
/// frontend's `library-changed` push, it carries whatever the toast needs to
/// describe itself, and it is deliberately absent from
/// `contract::types`' boundary list because it never crosses the IPC boundary.
/// `contract::events::Event` is the wire surface. One declaration of what
/// crosses, one internal callback for what does not — the thing WS2 exists to
/// prevent is two declarations of the *wire*, which this is not.
///
/// WS3 folds these together when notifications move into the daemon and this
/// seam becomes a socket write.
///
/// Type-erased for exactly the reason `on_event` is; see the field's comment.
type EventSink = Box<dyn Fn(crate::contract::events::Event) + Send + Sync>;

/// The seam for "this recording is written; go and find out what the LCU
/// says about the game it was".
///
/// Type-erased for exactly the reason `on_event` is, and it matters more
/// here: `stop_recording` is one of the few pieces of this file's async
/// glue that is directly unit-tested, and a bare
/// `tauri::async_runtime::spawn` in it would drag the Tauri runtime into a
/// code path `cargo test` executes. The tests leave this `None`, so
/// nothing spawns and the finalize path they drive is unchanged.
type SummaryFetcher = Box<dyn Fn(crate::match_summary::SummaryRequest) + Send + Sync>;

/// Takes a recording id and returns immediately. See `set_trim_requester`.
type TrimRequester = Box<dyn Fn(i64) + Send + Sync>;
/// Asks for any patches an app exit interrupted to be finished, now that a
/// client is reachable again (#137). Takes the lockfile because that is the
/// thing this module has and `match_summary` needs.
type SummaryResumer = Box<dyn Fn(lcu::LockfileInfo) + Send + Sync>;

/// Something the supervisor wants the rest of the app to know about.
///
/// A single enum behind a single notifier, rather than one callback per
/// signal: when the recorder moves into its own process this seam becomes a
/// socket write, and there should be exactly one place to change.
#[derive(Debug, Clone)]
pub enum SupervisorEvent {
    /// The VOD library changed behind the frontend's back.
    LibraryChanged,
    /// Capture began.
    RecordingStarted,
    /// A recording was finalized. Carries what was written so a notification
    /// can describe it without going back to the database.
    Finalized(FinalizedRecording),
    /// A recording could not be started or could not be finished. Carries a
    /// message fit to show the user — they are in a game and cannot see the
    /// window.
    RecordingFailed(String),
}

/// What the supervisor learned about the most recently finished recording,
/// surfaced to the frontend via `game_state_status`. Also written to the
/// SQLite VOD library (`db`) — `recording_id` is `None` only if that write
/// itself failed, so the in-memory copy still isn't lost.
#[derive(Debug, Clone, Serialize, ts_rs::TS)]
pub struct FinalizedRecording {
    pub recording_id: Option<i64>,
    pub path: String,
    pub markers: Vec<SessionMarker>,
}

#[derive(Debug, Clone, Serialize, ts_rs::TS)]
pub struct SupervisorStatus {
    pub state: GameState,
    pub last_finalized: Option<FinalizedRecording>,
    /// Seconds since capture actually started, or `None` when nothing is
    /// recording. Read from the session rather than timed by the UI: the
    /// window can be opened part-way through a game, and a counter that
    /// starts at zero when the UI first looks would misreport how much has
    /// been captured.
    pub recording_elapsed_s: Option<f64>,
}

/// What the app *observed* while making a recording, as against what the
/// recording contains.
///
/// Written to `recordings.diagnostics_json` at finalize (migration 7).
/// None of it survives the game otherwise: Live Client Data is gone the
/// moment the game ends, and a recording whose champion came out NULL, or
/// whose markers landed twenty seconds out, leaves nothing behind that
/// says why. `DevSessionView` carries some of the same numbers and is gone
/// on restart.
///
/// Deliberately not a copy of the row. Everything here is something the
/// columns cannot say.
#[derive(Debug, Clone, PartialEq, Serialize, serde::Deserialize, ts_rs::TS)]
pub struct RecordingDiagnostics {
    /// What the gameflow session said, and `game_id: None` is itself the
    /// answer to "why is queue NULL".
    pub game_id: Option<i64>,
    pub queue_id: Option<i64>,
    pub is_custom: bool,

    /// Successful Live Client Data polls. Not the same as `samples`, which
    /// only grow when the game clock advances — a gap between the two is a
    /// loading screen, a pause, or a stalled clock.
    pub polls: usize,
    pub first_game_time_s: Option<f64>,
    pub last_game_time_s: Option<f64>,
    /// Whether we were ever found in `allPlayers`. `false` is the whole
    /// explanation for a NULL champion, a NULL KDA and an empty advantage
    /// curve, and it is otherwise invisible — see `find_us`.
    pub ever_matched: bool,

    /// The game-time-to-video-time offset in force at the end, or `None`
    /// if the clock was never seen to advance. A marker that seeks to the
    /// wrong moment is this number being wrong.
    pub alignment_offset_s: Option<f64>,

    /// Which capture backend was actually live, which on Windows may be
    /// `FailedRecorder` carrying its init error.
    pub backend: String,

    /// What the finalize wrote. A count here that disagrees with the
    /// `markers`/`samples` tables means an insert failed.
    pub markers: usize,
    pub samples: usize,
}

struct RecordingSession {
    /// The `recordings` row opened when capture started, if it could be.
    ///
    /// `Some` is the normal case and is what lets markers be written as they
    /// arrive rather than at finalize (#150). `None` means the start-insert
    /// failed, which costs this recording the crash-safety and nothing else:
    /// the finalize falls back to `insert_recording` and behaves exactly as
    /// it did before this existed.
    recording_id: Option<i64>,
    tracker: MarkerTracker,
    markers: Vec<PendingMarker>,
    samples: Vec<PendingSample>,
    align: AlignmentTracker,
    /// Champion, KDA, game mode and outcome, accumulated across polls
    /// rather than read off the final one. `LiveSummary::absorb` explains
    /// why the last snapshot alone isn't enough — the poll carrying
    /// `GameEnd` is often the last one that ever succeeds.
    live: LiveSummary,
    /// The last poll that carried a player list, reduced to what the row
    /// draws.
    ///
    /// Last-good rather than last, for the same reason `LiveSummary`
    /// absorbs instead of replacing: the poll carrying `GameEnd` is often
    /// the last one that succeeds, and the ones after it — during the
    /// end-of-game screen, or as the process exits — come back with no
    /// `allPlayers` at all. Overwriting with one of those would trade a
    /// real scoreboard for the absence of one.
    scoreboard: Option<live_client::Scoreboard>,
    /// Diagnostic counters, for `RecordingDiagnostics` at finalize.
    polls: usize,
    first_game_time_s: Option<f64>,
    last_game_time_s: Option<f64>,
    ever_matched: bool,
    /// The gameflow session's answer, absorbed rather than re-read, so a
    /// client that goes away mid-game cannot take the identity back with it.
    game: lcu::GameIdentity,
    /// The last `LiveMatch` written to the open row, so a poll that
    /// establishes nothing new costs no write. Most polls do not: a KDA moves
    /// on a kill, not on a tick.
    last_live_written: Option<db::LiveMatch>,
    record_started_at: Instant,
    /// The stem `Recorder::start` was given. Kept rather than re-derived from
    /// `started_at_millis`: the format lives at the one call site that builds
    /// the `RecordConfig`, and a second copy of it here would be a second
    /// place to change when the naming does.
    #[cfg_attr(not(test), allow(dead_code))]
    file_stem: String,
    /// Wall-clock capture alongside `record_started_at` — `Instant` is
    /// monotonic only, not convertible to a real timestamp, but the DB's
    /// `recordings.started_at` column needs one.
    started_at_millis: i64,
}

impl RecordingSession {
    /// Folds one Live Client Data poll into the session: updates the match
    /// summary the finalize writes, updates the game-time-to-video-time
    /// alignment, collects any markers new since the last poll, and appends
    /// an advantage-curve sample.
    ///
    /// `elapsed_s` is how long capture had been running when this poll
    /// landed, passed in rather than read from `record_started_at` so tests
    /// can drive a whole game — loading screen, pauses and all — without
    /// waiting for one.
    /// Returns the markers this poll produced, already resolved against the
    /// alignment known *now*.
    ///
    /// **Provisional, and deliberately so.** `video_time_s` is computed from
    /// the current alignment or its fallback, and a later poll can improve
    /// that — so a marker published live may sit a fraction of a second from
    /// where the same marker lands in the database at finalize, which resolves
    /// every marker against the final alignment. The live value is for drawing
    /// a timeline while the game runs; the row is what the library reads.
    fn ingest(&mut self, snapshot: &AllGameData, elapsed_s: f64) -> Vec<SessionMarker> {
        // Before the early-exit-free alignment work below, because it has no
        // preconditions: champion and mode are readable on the very first
        // poll, and the outcome on whichever poll happens to carry `GameEnd`.
        let summary = live_client::self_summary(snapshot);
        // `kda` is `Some` exactly when we were found in `allPlayers`, which
        // is the one thing about a poll that is otherwise unrecoverable
        // afterwards.
        self.ever_matched |= summary.kda.is_some();
        self.live.absorb(summary);
        if let Some(scoreboard) = live_client::scoreboard(snapshot) {
            self.scoreboard = Some(scoreboard);
        }

        self.polls += 1;
        self.first_game_time_s
            .get_or_insert(snapshot.game_data.game_time);
        self.last_game_time_s = Some(snapshot.game_data.game_time);

        let game_time_s = snapshot.game_data.game_time;
        // `None` until the clock is first seen to advance. Markers stamped
        // with it are resolved against the fallback at finalize rather than
        // being discarded — bailing out early here would also skip
        // `tracker.ingest`, so those events would never be deduped and would
        // reappear as duplicates on the next poll.
        let alignment = self.align.observe(game_time_s, elapsed_s);

        let fresh = self.tracker.ingest(snapshot);
        // One line per poll, at debug: 1 Hz would bury the log at any
        // higher level, but it is the only record of what the app was
        // seeing when a game went wrong (#70).
        debug!(
            "live-poll",
            "{}",
            live_client::poll_trace(snapshot, elapsed_s, alignment, fresh.len())
        );
        let first_new = self.markers.len();
        self.markers
            .extend(fresh.into_iter().map(|marker| PendingMarker { marker, alignment }));
        let fallback = self.align.fallback();
        let added: Vec<SessionMarker> = self.markers[first_new..]
            .iter()
            .map(|m| m.resolve(fallback))
            .collect();

        // Advantage-curve sample. Skipped unless game time actually moved:
        // the poller re-fetches the same payload during loading screens and
        // pauses, and a flat run of identical timestamps would draw a
        // vertical artefact through the graph.
        let moved = self
            .samples
            .last()
            .is_none_or(|last| game_time_s > last.game_time_s);
        if moved {
            let active = snapshot.active_player.as_ref();
            self.samples.push(PendingSample {
                game_time_s,
                diff: team_diff(snapshot),
                our_gold: active.map(|p| p.current_gold).unwrap_or(0.0),
                our_level: active.map(|p| p.level).unwrap_or(0),
                alignment,
            });
        }

        added
    }

    /// Markers with their video positions resolved. Called at finalize, and
    /// by the dev portal's live readout.
    fn resolved_markers(&self) -> Vec<SessionMarker> {
        let fallback = self.align.fallback();
        self.markers.iter().map(|m| m.resolve(fallback)).collect()
    }

    /// Samples with their video positions resolved. See `resolved_markers`.
    fn resolved_samples(&self) -> Vec<SessionSample> {
        let fallback = self.align.fallback();
        self.samples.iter().map(|s| s.resolve(fallback)).collect()
    }

    /// What the polls have established so far, shaped for the row.
    ///
    /// `cs` comes from the scoreboard rather than from `LiveSummary`, which
    /// is the one field here that does: it is the only part of the scoreboard
    /// with a column of its own, because it is shown on the card and sorted
    /// by.
    fn live_row(&self) -> db::LiveMatch {
        db::LiveMatch {
            champion: self.live.champion.clone(),
            role: self.live.role.clone(),
            win: self.live.win,
            kda_k: self.live.kda.map(|k| k.kills),
            kda_d: self.live.kda.map(|k| k.deaths),
            kda_a: self.live.kda.map(|k| k.assists),
            game_mode: self.live.game_mode.clone(),
            cs: self
                .scoreboard
                .as_ref()
                .and_then(|s| s.players.iter().find(|p| p.is_us))
                .map(|p| p.cs),
            game_id: self.game.game_id,
            queue: self.game.queue_id,
        }
    }

    /// The sample this poll produced, if it produced one.
    ///
    /// `ingest` pushes at most one and already has fifteen call sites, so the
    /// count taken before it is what says whether it did, rather than a
    /// changed return type.
    fn sample_added_since(&self, count_before: usize) -> Option<SessionSample> {
        let fallback = self.align.fallback();
        self.samples.get(count_before).map(|s| s.resolve(fallback))
    }
}

pub struct Supervisor {
    machine: Mutex<StateMachine>,
    recorder: Arc<Mutex<Box<dyn Recorder>>>,
    recordings_dir: PathBuf,
    db: Arc<Db>,
    gameflow_task: Mutex<Option<JoinHandle<()>>>,
    live_client_task: Mutex<Option<JoinHandle<()>>>,
    session: Mutex<Option<RecordingSession>>,
    /// The lockfile behind the gameflow watch currently running, so an LCU
    /// request can be made from outside that watcher's task. Stashed when
    /// the watch starts rather than rediscovered on demand: the client can
    /// restart mid-session and `discover` would then answer about a
    /// different process than the one we are tracking.
    lockfile: Mutex<Option<lcu::LockfileInfo>>,
    /// Which game the client says is running, read once per game from
    /// `/lol-gameflow/v1/session`.
    ///
    /// Deliberately on the supervisor rather than on `RecordingSession`:
    /// the read happens when gameflow reaches `InProgress`, and recording
    /// does not begin until Live Client Data answers a loading screen
    /// later, so there is no session to put it in yet. Reading it at
    /// finalize also sidesteps the race entirely — by then the request has
    /// long since resolved either way.
    pending_game: Mutex<lcu::GameIdentity>,
    /// The ladder as it stood when this game started (#164).
    ///
    /// Read here rather than at finalize because that is the only moment it
    /// is true: it is the *before* half of a measurement, and by the time the
    /// game ends the number has already moved. Held beside `pending_game`
    /// because it is resolved from the same session read and has the same
    /// lifetime — one game's worth.
    rank_before: Mutex<Option<lcu::ranked::Standing>>,
    last_finalized: Mutex<Option<FinalizedRecording>>,
    /// Set once at startup via `set_event_notifier`, rather
    /// than taken in `new`, so the unit tests below can still build a
    /// `Supervisor` without a Tauri runtime. `None` simply means nothing
    /// is emitted.
    ///
    /// Deliberately a boxed closure rather than an `AppHandle`. Holding
    /// the handle here and calling `Emitter::emit` on it made Tauri's Wry
    /// window machinery *reachable* from this module — and this module has
    /// unit tests, so the linker could no longer discard it from the test
    /// harness. That dragged the whole Win32 GUI stack (`user32`, `gdi32`,
    /// `comctl32`, `ole32`, `shell32`, …) into the test executable's
    /// import table, and a `cargo test` binary carries no application
    /// manifest — so Windows resolved `comctl32.dll` to the v5
    /// side-by-side assembly, which lacks the v6 exports Tauri links
    /// against. The test binary then died at load with
    /// STATUS_ENTRYPOINT_NOT_FOUND before running a single test.
    /// Type-erasing the emit keeps all of that inside `lib.rs`'s `run()`,
    /// which stays dead code — and so gets stripped — in a test build.
    on_event: Mutex<Option<EventNotifier>>,
    /// The contract-event sink. Installed from `lib.rs` like `on_event`, and
    /// `None` in every unit test that does not deliberately install one, so the
    /// paths below behave exactly as they did before this existed.
    on_contract_event: Mutex<Option<EventSink>>,
    /// Epoch milliseconds at which the current state was entered, so
    /// `Event::StateChanged` can carry `since_ms` without a client timing it
    /// itself. Set by `dispatch_one` on every transition that actually moves.
    state_since_ms: Mutex<i64>,
    /// Set once at startup from `lib.rs`, like `on_event`. `None` means a
    /// finalized recording keeps whatever Live Client Data established and
    /// is never revisited — which is what every unit test below wants, and
    /// what a build with no League client running gets anyway.
    summary_fetcher: Mutex<Option<SummaryFetcher>>,
    summary_resumer: Mutex<Option<SummaryResumer>>,
    /// Asks for the loading screen to be cut off a finished recording
    /// (`crate::trim`).
    ///
    /// Type-erased and installed from `lib.rs` for the same two reasons as
    /// `summary_fetcher`: this module stays free of the async runtime, and
    /// left `None` a finalize spawns no process and rewrites no file, which
    /// is what every unit test below wants.
    trim_requester: Mutex<Option<TrimRequester>>,
}

impl Supervisor {
    pub fn new(
        recorder: Arc<Mutex<Box<dyn Recorder>>>,
        recordings_dir: PathBuf,
        db: Arc<Db>,
    ) -> Arc<Self> {
        Arc::new(Self {
            machine: Mutex::new(StateMachine::new()),
            recorder,
            recordings_dir,
            db,
            rank_before: Mutex::new(None),
            gameflow_task: Mutex::new(None),
            live_client_task: Mutex::new(None),
            session: Mutex::new(None),
            lockfile: Mutex::new(None),
            pending_game: Mutex::new(lcu::GameIdentity::default()),
            last_finalized: Mutex::new(None),
            on_event: Mutex::new(None),
            on_contract_event: Mutex::new(None),
            state_since_ms: Mutex::new(timestamp_millis()),
            summary_fetcher: Mutex::new(None),
            summary_resumer: Mutex::new(None),
            trim_requester: Mutex::new(None),
        })
    }

    /// Gives the supervisor a way to tell the frontend the library
    /// changed. Called once from `lib.rs`'s `setup`, after the app is
    /// built. See `on_event` for why this takes a closure and not an
    /// `AppHandle`.
    ///
    /// One notifier over an event enum rather than one per signal: the
    /// process split will replace what sits behind this seam with a socket
    /// write, and having a single seam to replace is the point.
    pub fn set_event_notifier(&self, notify: EventNotifier) {
        *self.on_event.lock().unwrap() = Some(notify);
    }

    /// Gives the supervisor somewhere to publish contract events. Called once
    /// from `lib.rs`'s `setup`, beside `set_event_notifier`, and for the same
    /// reason type-erased rather than handed an `AppHandle`.
    pub fn set_event_sink(&self, sink: EventSink) {
        *self.on_contract_event.lock().unwrap() = Some(sink);
    }

    /// Gives the supervisor somewhere to send post-game summary requests.
    /// Called once from `lib.rs`'s `setup`, alongside `set_event_notifier`.
    ///
    /// Whatever is installed here **must return immediately** — it is
    /// called from inside `stop_recording`, under the recorder lock. The
    /// real one spawns a task and returns; see `crate::match_summary`.
    /// Same shape and the same reason as `set_summary_fetcher`: the work
    /// needs the async runtime, and this module stays out of it.
    pub fn set_summary_resumer(&self, resume: SummaryResumer) {
        *self.summary_resumer.lock().unwrap() = Some(resume);
    }

    pub fn set_summary_fetcher(&self, fetch: SummaryFetcher) {
        *self.summary_fetcher.lock().unwrap() = Some(fetch);
    }

    /// Gives the supervisor somewhere to send a finished recording to have
    /// its loading screen cut off. Called once from `lib.rs`'s `setup`.
    ///
    /// **Whatever is installed here must return immediately**, exactly like
    /// `set_summary_fetcher`, and for a sharper reason: this is called from
    /// inside `stop_recording` under the recorder lock, and the work behind
    /// it is a stream copy of a file that can be gigabytes. Done inline it
    /// would hold that lock for as long as the copy takes — blocking
    /// `is_recording`, which the header polls once a second, and
    /// `start_recording`, which is how the *next* game begins.
    pub fn set_trim_requester(&self, request: TrimRequester) {
        *self.trim_requester.lock().unwrap() = Some(request);
    }

    /// Asks for the loading screen to be cut off the recording just
    /// written, and returns.
    ///
    /// Sent *after* the markers and samples are in, because the trim is
    /// expressed as a rebase of exactly those rows — the same path
    /// `dev_trim_lead_in` takes for a recording made before this existed.
    fn request_trim(&self, recording_id: i64) {
        if let Some(request) = self.trim_requester.lock().unwrap().as_ref() {
            request(recording_id);
        }
    }

    fn emit(&self, event: SupervisorEvent) {
        if let Some(notify) = self.on_event.lock().unwrap().as_ref() {
            notify(event);
        }
    }

    /// Publishes one contract event. A no-op when no sink is installed, which
    /// is what every unit test that does not care about events gets.
    ///
    /// The sink runs **under the lock**, exactly as `emit` runs its notifier —
    /// so a sink that called back into the supervisor would deadlock. The one
    /// installed from `lib.rs` emits a Tauri event and returns, like the
    /// notifier beside it.
    fn publish(&self, event: crate::contract::events::Event) {
        if let Some(publish) = self.on_contract_event.lock().unwrap().as_ref() {
            publish(event);
        }
    }

    /// Tells the frontend the VOD library changed on disk. Until this
    /// existed the app had no backend-to-frontend push at all, so a
    /// recording finalized by the supervisor stayed invisible until the
    /// user happened to press Refresh.
    fn emit_library_changed(&self, reason: crate::contract::events::LibraryChangeReason) {
        self.emit(SupervisorEvent::LibraryChanged);
        self.publish(crate::contract::events::Event::LibraryChanged { reason });
    }

    pub fn status(&self) -> SupervisorStatus {
        SupervisorStatus {
            state: self.machine.lock().unwrap().state.clone(),
            last_finalized: self.last_finalized.lock().unwrap().clone(),
            recording_elapsed_s: self
                .session
                .lock()
                .unwrap()
                .as_ref()
                .map(|s| s.record_started_at.elapsed().as_secs_f64()),
        }
    }

    /// The recording in flight, reduced to what a fresh client needs to
    /// render one.
    ///
    /// Shares its numbers with `dev_session_view` but is not behind
    /// `devtools`: a snapshot has to describe an in-progress game in a shipped
    /// build, which is exactly the case a UI killed mid-game comes back to.
    /// `None` is the resting state, not a failure.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn current_recording(&self) -> Option<crate::contract::snapshot::CurrentRecording> {
        let guard = self.session.lock().unwrap();
        let session = guard.as_ref()?;
        Some(crate::contract::snapshot::CurrentRecording {
            file_stem: session.file_stem.clone(),
            started_at_millis: session.started_at_millis,
            elapsed_s: session.record_started_at.elapsed().as_secs_f64(),
            marker_count: session.markers.len(),
            sample_count: session.samples.len(),
            alignment_offset_s: session.align.current_offset_s(),
        })
    }

    /// Starts the always-on lockfile watch. Everything else (gameflow
    /// watch, Live Client Data polling, recording) is started/stopped by
    /// state transitions from here on. Runs for the app's lifetime.
    /// Finalizes an in-flight recording so the process can exit without
    /// losing the game, returning whether there was one.
    ///
    /// The tray's Quit needs this: quitting mid-match would otherwise leave a
    /// fragmented MP4 on disk with no `recordings` row, recoverable only by
    /// the next startup's `reconcile` and stripped of its markers.
    ///
    /// **Blocking.** This is the same finalize the state machine runs — the
    /// recorder stop, the ffmpeg remux, the DB write and a retention sweep —
    /// so it must not be called from the main thread, which on the tray path
    /// would freeze the menu and the whole event loop for seconds.
    pub fn finalize_for_shutdown(&self) -> bool {
        if self.session.lock().unwrap().is_none() {
            return false;
        }
        self.stop_recording();
        true
    }

    pub fn start(self: &Arc<Self>) {
        let sup = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            lcu::lockfile::watch(Duration::from_secs(2), move |state| {
                sup.dispatch(StateEvent::LockfileChanged(state));
            })
            .await;
        });
    }

    /// Feeds one event through the state machine and executes whatever
    /// actions it returns, then always tries `FinalizeComplete` as a
    /// follow-up. That's only a real transition when we just entered
    /// `Finalizing` (whose finalize actions — stop recorder, stop
    /// pollers — run synchronously above, so completing immediately is
    /// correct); in every other state it hits the state machine's
    /// wildcard arm and is a no-op. This blanket approach means callers
    /// never need to remember to send `FinalizeComplete` themselves, and
    /// avoids re-entrant locking that a nested `dispatch` call from
    /// inside `execute` would cause.
    fn dispatch(self: &Arc<Self>, event: StateEvent) {
        self.dispatch_one(event);
        self.dispatch_one(StateEvent::FinalizeComplete);
    }

    fn dispatch_one(self: &Arc<Self>, event: StateEvent) {
        let (actions, state, moved) = {
            let mut machine = self.machine.lock().unwrap();
            let before = machine.state.clone();
            let actions = machine.handle(event);
            let after = machine.state.clone();
            (actions, after.clone(), before != after)
        };
        // **One event per transition, not per handled event.** `dispatch` runs
        // this twice — the caller's event, then a blanket `FinalizeComplete`
        // that no-ops everywhere but `Finalizing` — and gameflow re-reports the
        // phase it is already in, so emitting on every `handle` would push a
        // redraw at 1 Hz for a state nobody moved out of.
        if moved {
            let since_ms = timestamp_millis();
            *self.state_since_ms.lock().unwrap() = since_ms;
            self.publish(crate::contract::events::Event::StateChanged {
                state: state.clone(),
                since_ms,
            });
        }
        for action in actions {
            self.execute(action);
        }
        // Idle means the client is gone, so the stashed lockfile now
        // describes a dead process and any request through it would be
        // refused. Dropping it makes "is there a client" answerable
        // without making one.
        if matches!(state, GameState::Idle) {
            *self.lockfile.lock().unwrap() = None;
        }
        self.sync_capture_backend(&state);
    }

    /// Keeps the capture backend's resident cost tied to whether a game is
    /// plausible: warm from the moment the League client is running, cold
    /// once it isn't (DEVELOPMENT.md §1.2).
    ///
    /// Driven off the resulting *state* rather than off `Action`s, because
    /// the actions don't say it. A client restart emits a stop and a start
    /// of the gameflow watch while staying in `ClientRunning`, so acting on
    /// those would tear libobs down and bring it straight back up for
    /// nothing. Reading the state makes that a no-op, and makes this
    /// idempotent — which matters because `dispatch` runs us twice.
    fn sync_capture_backend(&self, state: &GameState) {
        // Blocks on the recorder mutex, which `stop_recording` holds for the
        // whole finalize. So a lockfile-watch dispatch landing mid-finalize
        // waits a few seconds here. Deliberate: `try_lock` would be
        // non-blocking but could silently skip the `release` at `Idle`, and
        // nothing would dispatch again until the client came back, leaving
        // the backend warm indefinitely — the exact thing this exists to
        // prevent. Delaying a background poll is the cheaper trade.
        let mut recorder = self.recorder.lock().unwrap();
        if matches!(state, GameState::Idle) {
            recorder.release();
        } else if let Err(e) = recorder.prepare() {
            // Not fatal: `start` retries the bring-up itself, and a warning
            // now is more useful than silence until someone tries to record.
            warn!("state_machine", "capture backend not ready: {e}");
        }
    }

    fn execute(self: &Arc<Self>, action: Action) {
        match action {
            Action::StartGameflowWatch(info) => self.start_gameflow_watch(info),
            Action::StopGameflowWatch => self.stop_gameflow_watch(),
            Action::StartLiveClientPoll => self.start_live_client_poll(),
            Action::StopLiveClientPoll => self.stop_live_client_poll(),
            Action::StartRecording => self.start_recording(),
            Action::StopRecording => self.stop_recording(),
        }
    }

    fn start_gameflow_watch(self: &Arc<Self>, lockfile: lcu::LockfileInfo) {
        *self.lockfile.lock().unwrap() = Some(lockfile.clone());

        // A client just became reachable, which is the only moment an
        // interrupted patch can be finished. Deliberately here rather than at
        // startup: the app can start long before League does, and a sweep
        // that ran with no client would find nothing and never run again.
        if let Some(resume) = self.summary_resumer.lock().unwrap().as_ref() {
            resume(lockfile.clone());
        }

        let sup = Arc::clone(self);
        let handle = tauri::async_runtime::spawn(async move {
            let client = match lcu::LcuHttpClient::new(&lockfile) {
                Ok(c) => c,
                Err(e) => {
                    warn!("state_machine", "failed to build LCU client: {e}");
                    return;
                }
            };
            lcu::gameflow::watch(&lockfile, &client, Duration::from_secs(1), {
                let sup = Arc::clone(&sup);
                move |update| {
                    // Published before dispatching, so a client sees the phase
                    // that *caused* a transition ahead of the transition
                    // itself. The watcher already de-duplicates, so this is one
                    // event per actual phase change rather than one per poll.
                    sup.publish(crate::contract::events::Event::LcuPhase {
                        phase: Some(update.phase.clone()),
                        client_present: true,
                    });
                    sup.dispatch(StateEvent::GameflowPhase(update.phase))
                }
            })
            .await;
        });
        *self.gameflow_task.lock().unwrap() = Some(handle);
    }

    fn stop_gameflow_watch(&self) {
        if let Some(handle) = self.gameflow_task.lock().unwrap().take() {
            handle.abort();
        }
        // The other half of `LcuPhase`, and the reason `phase` is an `Option`:
        // with no client there is no phase, which is a different statement from
        // `GameflowPhase::None` — a client sitting at the front page.
        self.publish(crate::contract::events::Event::LcuPhase {
            phase: None,
            client_present: false,
        });
        // The lockfile is deliberately *not* cleared here. Finalize stops
        // the watch before the recording row is written, and the client is
        // usually still running — dropping it would take the LCU out of
        // reach exactly when the post-game summary needs it.
    }

    /// Asks the client which game is starting and remembers the answer for
    /// the finalize.
    ///
    /// Best-effort by design: a game we cannot identify still records, it
    /// just lands without a `game_id` or `queue`. Failing here must never
    /// stop a recording — losing the footage over a missing queue label
    /// would be an absurd trade.
    async fn fetch_game_identity(&self) {
        let lockfile = { self.lockfile.lock().unwrap().clone() };
        let Some(lockfile) = lockfile else {
            // No gameflow watch is running, so nothing told us a game
            // started — the dev portal's manual start does this.
            return;
        };

        let client = match lcu::LcuHttpClient::new(&lockfile) {
            Ok(c) => c,
            Err(e) => {
                warn!("state_machine", "could not build an LCU client to identify the game: {e}");
                return;
            }
        };

        match lcu::fetch_session(&client).await {
            Ok(identity) => {
                // Logged rather than silent: this is the one line that
                // tells a Windows tester the id resolution works, and
                // `is_custom` is why a later summary fetch may find
                // nothing (custom games never reach match history).
                info!(
                    "state_machine",
                    "game identified: id={:?} queue={:?} custom={}",
                    identity.game_id, identity.queue_id, identity.is_custom
                );
                // The ladder this game starts from, if it has one. Best
                // effort and after the identity is stored: a standing that
                // cannot be read costs a delta, while the identity it is
                // keyed by is what the whole row depends on.
                if let Some(queue_type) = identity
                    .queue_id
                    .and_then(lcu::ranked::queue_type_for)
                {
                    match client
                        .get_json::<lcu::ranked::RankedStats>(
                            "/lol-ranked/v1/current-ranked-stats",
                        )
                        .await
                    {
                        Ok(stats) => {
                            let standing = stats.standing(queue_type);
                            info!("state_machine", "starting from {}",
                                match &standing {
                                    Some(s) => format!("{} {} ({} LP)",
                                        s.tier, s.division.as_deref().unwrap_or(""), s.league_points),
                                    None => "no standing in this queue — unranked, or placements".into(),
                                }
                            );
                            *self.rank_before.lock().unwrap() = standing;
                        }
                        Err(e) => warn!("state_machine",
                            "could not read the ladder this game starts from: {e}"),
                    }
                }
                *self.pending_game.lock().unwrap() = identity;
            }
            Err(e) => warn!("state_machine", "could not identify the game: {e}"),
        }
    }

    fn start_live_client_poll(self: &Arc<Self>) {
        let sup = Arc::clone(self);
        let handle = tauri::async_runtime::spawn(async move {
            // A new game starts here, so whatever the last one resolved to
            // must not leak into this recording's row.
            *sup.pending_game.lock().unwrap() = lcu::GameIdentity::default();
            // Same reason as the identity above: last game's standing must
            // not become this game's *before*, which would measure the wrong
            // interval entirely.
            *sup.rank_before.lock().unwrap() = None;
            sup.fetch_game_identity().await;

            let client = match live_client::LiveClientDataClient::new() {
                Ok(c) => c,
                Err(e) => {
                    error!("state_machine", "failed to build Live Client Data client: {e}");
                    return;
                }
            };
            live_client::poller::watch(
                &client,
                Duration::from_secs(1),
                {
                    let sup = Arc::clone(&sup);
                    move |snapshot| sup.on_snapshot(snapshot)
                },
                {
                    let sup = Arc::clone(&sup);
                    move || sup.dispatch(StateEvent::LiveClientDown)
                },
            )
            .await;
        });
        *self.live_client_task.lock().unwrap() = Some(handle);
    }

    fn stop_live_client_poll(&self) {
        if let Some(handle) = self.live_client_task.lock().unwrap().take() {
            handle.abort();
        }
    }

    /// Every successful poll: (1) tells the state machine Live Client Data
    /// is reachable — a no-op unless we're still `WaitingForGame`, in
    /// which case it starts recording; (2) if we're recording, hands the
    /// snapshot to the session.
    ///
    /// The elapsed-time read is the only thing this does beyond locking and
    /// delegating: `RecordingSession::ingest` takes it as an argument so the
    /// summary/marker/sample/alignment logic is a pure function of its
    /// inputs and can be unit-tested without a clock or a live game
    /// (CLAUDE.md: pure decision, thin I/O wrapper).
    fn on_snapshot(self: &Arc<Self>, snapshot: AllGameData) {
        self.dispatch(StateEvent::LiveClientUp);

        let mut guard = self.session.lock().unwrap();
        let Some(session) = guard.as_mut() else {
            // Polls before capture begins. Traced too: "the endpoint was
            // answering for 40s before recording started" is the answer to
            // a recording that begins late.
            debug!(
                "live-poll",
                "game={:.1} waiting (not recording yet)", snapshot.game_data.game_time
            );
            return;
        };
        let elapsed_s = session.record_started_at.elapsed().as_secs_f64();
        let samples_before = session.samples.len();
        let added = session.ingest(&snapshot, elapsed_s);
        let new_sample = session.sample_added_since(samples_before);
        // The identity is fetched once per game by a task that races the
        // first polls, so it is picked up here rather than at `start`, and
        // absorbed rather than assigned: a client that drops out later must
        // not be able to hand back an id this recording already read.
        session.game.absorb(*self.pending_game.lock().unwrap());
        // Only when this poll actually established something. A KDA moves on
        // a kill rather than on a tick, so most polls leave this `None` and
        // cost no write at all; the loading screen and the end-of-game screen
        // leave it `None` for every poll they produce.
        let live_row = {
            let row = session.live_row();
            (session.last_live_written.as_ref() != Some(&row)).then_some(row)
        };
        let recording_id = session.recording_id;
        // The session lock is dropped before publishing: `publish` runs the
        // sink inline, and holding this across it would put a `lib.rs` closure
        // inside the lock that every poll and the whole finalize contend for.
        drop(guard);

        // **Written now, not at finalize** (#150). This is the whole fix: a
        // daemon killed from here on leaves these markers in the database
        // attached to a real row, instead of taking every one of them with it.
        //
        // Positions are provisional in the same way the published event below
        // is provisional, and for the same reason: a marker seen during the
        // loading screen, before game time ever advanced, resolves against a
        // 1:1 fallback that a later poll improves on. The finalize deletes
        // these and re-inserts them against the final alignment, so the
        // durable answer is never worse than it was before. What changes is
        // that there is an answer at all when the finalize never runs.
        //
        // Only when there were markers: a quiet poll must not open a write
        // transaction, and at 1 Hz for a 30-minute game most polls are quiet.
        if let Some(id) = recording_id
            && !added.is_empty()
        {
            let rows: Vec<db::NewMarker> = added.iter().map(marker_row).collect();
            if let Err(e) = self.db.insert_markers(id, &rows) {
                // Logged, never fatal. The in-memory copy is still there and
                // the finalize will write it, which is exactly the behaviour
                // this replaced.
                warn!(
                    "state_machine",
                    "could not write {} live marker(s) for recording {id}: {e}",
                    rows.len()
                );
            }
        }

        // The same fix as the markers above, for the curve: #150 covered only
        // half of this, and #185 is the other half. A killed daemon used to
        // leave a recovered recording with its markers on the timeline and an
        // empty graph underneath them, because samples were written only at
        // finalize.
        //
        // One row per poll at 1 Hz, and only when game time moved, so this is
        // a write a second during a game and nothing at all on a loading
        // screen. Its position is provisional exactly as a marker's is, and
        // the finalize deletes and re-inserts against the final alignment.
        //
        // Logged, never fatal, for the same reason as the markers: the
        // in-memory copy survives and the finalize will write it.
        if let Some(id) = recording_id
            && let Some(sample) = new_sample.as_ref()
            && let Err(e) = self.db.insert_samples(id, &[sample_row(sample)])
        {
            warn!(
                "state_machine",
                "could not write a live sample for recording {id}: {e}"
            );
        }

        // The third thing a killed daemon used to lose, after the markers
        // (#150) and the curve (#185); this half is #190. Champion, KDA, mode
        // and outcome were held in `session.live` for the whole game and
        // written once, at finalize, so a recovered recording came back as a
        // card with no title. The polls knew: nothing had asked them.
        //
        // Not a `COALESCE` merge like the LCU's later patch. This is the live
        // client writing the row it owns while it owns it, and `absorb` never
        // hands back a field it once knew, so a plain overwrite cannot lose
        // anything. The finalize rewrites all of it regardless.
        if let Some(id) = recording_id
            && let Some(row) = live_row
        {
            match self.db.update_live_summary(id, &row) {
                // Remembered only once it is actually in the database. A write
                // that failed has to be retried, and the next poll will retry
                // it without being told to, because the comparison above is
                // still looking at the last value that landed.
                Ok(()) => {
                    if let Some(session) = self.session.lock().unwrap().as_mut()
                        && session.recording_id == Some(id)
                    {
                        session.last_live_written = Some(row);
                    }
                }
                Err(e) => warn!(
                    "state_machine",
                    "could not write the live summary for recording {id}: {e}"
                ),
            }
        }

        for marker in added {
            // `None` even though a row now exists. The row is deliberately not
            // a library entry until it is finished, so handing out its id
            // would invite a client to look up something `list_recordings`
            // hides. Correlation stays what it was: the `RecordingStarted`
            // the client has already seen, and its `file_stem`.
            self.publish(crate::contract::events::Event::MarkerAdded {
                recording_id: None,
                marker,
            });
        }
    }

    /// Executes `Action::StartRecording`. The state machine has already
    /// optimistically transitioned to `Recording` by the time this runs —
    /// if `Recorder::start` fails here (only reachable today via the dev
    /// panel's manual start button racing the automatic path, since
    /// nothing else calls it), the state machine's belief and the
    /// recorder's actual state diverge. Known gap, logged loudly rather
    /// than silently wrong; not expected to occur outside that manual
    /// double-start collision.
    fn start_recording(&self) {
        if !crate::retention::has_room_to_record(&self.recordings_dir) {
            error!("state_machine", "refusing to start recording: insufficient free disk space");
            self.emit(SupervisorEvent::RecordingFailed(
                "not enough free disk space to record this game".into(),
            ));
            self.publish(crate::contract::events::Event::RecordingStopped {
                recording_id: None,
                outcome: crate::contract::events::StopOutcome::Refused {
                    reason: "not enough free disk space to record this game".into(),
                },
            });
            return;
        }

        let started_at_millis = timestamp_millis();
        // Read the preset per recording rather than caching it at startup:
        // the user can change what gets captured between games, and the
        // next game should honour that without a restart.
        // Kept rather than read back off `config`: `start` consumes it, and
        // this is the only correlation key a client has until finalize.
        let config_file_stem = format!("recording-{started_at_millis}");
        let config = RecordConfig {
            output_dir: self.recordings_dir.clone(),
            file_stem: config_file_stem.clone(),
            audio: self.db.get_audio_preset().unwrap_or_else(|e| {
                warn!("state_machine", "could not read the audio preset ({e}), using the default");
                Default::default()
            }),
        };
        // Read before `start` consumes the config. A prediction of where the
        // backend will write, which is enough for the row below: the finalize
        // corrects it by id from what `stop` actually reports.
        let expected_path = config.expected_output_path();
        match self.recorder.lock().unwrap().start(config) {
            Ok(()) => {
                // The row goes in now, unfinished, so that markers have
                // somewhere to go for the rest of the game (#150). Before
                // this, every marker lived in the `markers` vec below until
                // finalize, and a daemon killed mid-game took all of them
                // with it while leaving a perfectly playable file behind.
                //
                // Unfinished means `finished_at IS NULL`, which keeps it out
                // of the library and away from retention until a finalize or
                // a recovery pass completes it.
                //
                // A failure here is logged and carried: recording without
                // crash-safe markers is worth more than not recording.
                let recording_id = match self
                    .db
                    .begin_recording(&expected_path.display().to_string(), started_at_millis)
                {
                    Ok(id) => Some(id),
                    Err(e) => {
                        error!(
                            "state_machine",
                            "could not open a recording row ({e}); markers for this game will \
                             only be written at finalize"
                        );
                        None
                    }
                };
                *self.session.lock().unwrap() = Some(RecordingSession {
                    recording_id,
                    tracker: MarkerTracker::new(),
                    markers: Vec::new(),
                    samples: Vec::new(),
                    align: AlignmentTracker::new(),
                    live: LiveSummary::default(),
                    scoreboard: None,
                    polls: 0,
                    first_game_time_s: None,
                    last_game_time_s: None,
                    ever_matched: false,
                    game: lcu::GameIdentity::default(),
                    last_live_written: None,
                    record_started_at: Instant::now(),
                    file_stem: config_file_stem.clone(),
                    started_at_millis,
                });
                self.emit(SupervisorEvent::RecordingStarted);
                self.publish(crate::contract::events::Event::RecordingStarted {
                    recording_id: None,
                    file_stem: config_file_stem,
                    started_at_ms: started_at_millis,
                });
            }
            Err(e) => {
                error!("state_machine", "failed to start recording: {e}");
                // Silence here means the user finds out after the game, when
                // the VOD isn't in the library.
                self.emit(SupervisorEvent::RecordingFailed(format!(
                    "the recording could not be started: {e}"
                )));
                // `Crashed`, not `Refused`: the recorder was asked and failed.
                // A refusal is this module deciding not to record at all.
                self.publish(crate::contract::events::Event::RecordingStopped {
                    recording_id: None,
                    outcome: crate::contract::events::StopOutcome::Crashed,
                });
            }
        }
    }

    /// Executes `Action::StopRecording`: stops the recorder, then writes
    /// the recording + its markers to the VOD library DB (DEVELOPMENT.md
    /// §4). A DB write failure is logged but doesn't lose the in-memory
    /// copy — `last_finalized` is still set either way, just with
    /// `recording_id: None`, so nothing already captured is thrown away
    /// even if the row never made it to disk.
    fn stop_recording(&self) {
        let session = self.session.lock().unwrap().take();
        // Read *before* the `match` below: that match holds the recorder
        // lock for the whole of its body (the guard is a temporary in the
        // scrutinee), and `std::sync::Mutex` is not reentrant, so asking
        // the recorder anything inside it would deadlock the finalize.
        let backend = self.recorder.lock().unwrap().backend_name();
        // Read the clock here rather than after `stop()`: stopping runs the
        // recorder's shutdown and ffmpeg remux, which takes seconds on a long
        // game, and every one of them would be counted as footage the library
        // claims the file contains.
        let duration_s = session
            .as_ref()
            .map(|s| s.record_started_at.elapsed().as_secs_f64());
        match self.recorder.lock().unwrap().stop() {
            Ok(output) => {
                let path = output.path;
                // Video positions are computed here, not at ingest: the
                // alignment is only fully known once the game is over.
                let markers = session
                    .as_ref()
                    .map(|s| s.resolved_markers())
                    .unwrap_or_default();
                let samples = session
                    .as_ref()
                    .map(|s| s.resolved_samples())
                    .unwrap_or_default();
                let started_at = session
                    .as_ref()
                    .map(|s| s.started_at_millis)
                    .unwrap_or_else(timestamp_millis);
                // Whatever the Live Client Data polls managed to establish.
                // Every field is independently optional, so a game that
                // ended before the poller ever came up still writes a row —
                // just an emptier one, exactly as it does today.
                let live = session.as_ref().map(|s| s.live.clone()).unwrap_or_default();
                let scoreboard = session.as_ref().and_then(|s| s.scoreboard.clone());
                // Read from the supervisor, not the session: the identity
                // is resolved when gameflow reaches InProgress, which is
                // before this recording's session existed.
                // The session's copy first, because it absorbed every read
                // taken while the game ran; the supervisor's is what a
                // recording with no successful poll has instead. Neither can
                // erase the other, which is the point of `absorb`.
                let game = {
                    let mut identity = session.as_ref().map(|s| s.game).unwrap_or_default();
                    identity.absorb(*self.pending_game.lock().unwrap());
                    identity
                };
                let path_str = path.display().to_string();
                let size_bytes = std::fs::metadata(&path).map(|m| m.len() as i64).unwrap_or(0);

                // Serialized from what the backend reports it wrote, not
                // from the preset we asked for — see `RecordingOutput`.
                let audio_tracks_json = match serde_json::to_string(&output.audio) {
                    Ok(json) => Some(json),
                    Err(e) => {
                        warn!("state_machine", "could not encode the audio track layout: {e}");
                        None
                    }
                };

                // What we saw while recording, as against what the file
                // holds. Nothing else keeps any of it (#71).
                let diagnostics = RecordingDiagnostics {
                    game_id: game.game_id,
                    queue_id: game.queue_id,
                    is_custom: game.is_custom,
                    polls: session.as_ref().map(|s| s.polls).unwrap_or(0),
                    first_game_time_s: session.as_ref().and_then(|s| s.first_game_time_s),
                    last_game_time_s: session.as_ref().and_then(|s| s.last_game_time_s),
                    ever_matched: session.as_ref().is_some_and(|s| s.ever_matched),
                    alignment_offset_s: session.as_ref().and_then(|s| s.align.current_offset_s()),
                    backend,
                    markers: markers.len(),
                    samples: samples.len(),
                };
                let diagnostics_json = match serde_json::to_string(&diagnostics) {
                    Ok(json) => Some(json),
                    // A row without diagnostics is worth strictly more than
                    // no row, so this never fails the finalize.
                    Err(e) => {
                        warn!("state_machine", "could not encode the recording diagnostics: {e}");
                        None
                    }
                };

                let row = db::NewRecording {
                    path: path_str.clone(),
                    started_at,
                    duration_s,
                    game_id: game.game_id,
                    queue: game.queue_id,
                    champion: live.champion.clone(),
                    role: live.role.clone(),
                    win: live.win,
                    kda_k: live.kda.map(|k| k.kills),
                    kda_d: live.kda.map(|k| k.deaths),
                    kda_a: live.kda.map(|k| k.assists),
                    game_mode: live.game_mode.clone(),
                    size_bytes,
                    audio_tracks_json,
                    diagnostics_json,
                    // Serialization failing costs the scoreboard and
                    // nothing else, exactly as it does for the two blobs
                    // above — the row is written either way.
                    scoreboard_json: scoreboard
                        .as_ref()
                        .and_then(|s| serde_json::to_string(s).ok()),
                    cs: scoreboard
                        .as_ref()
                        .and_then(|s| s.players.iter().find(|p| p.is_us))
                        .map(|p| p.cs),
                    // **This is what puts the row in the library** (#150). A
                    // row exists from the moment recording starts so markers
                    // have somewhere to go; until this is set it is hidden,
                    // which is what keeps a half-written file and a recording
                    // a killed daemon abandoned out of the grid.
                    finished_at: Some(timestamp_millis()),
                    ..Default::default()
                };

                // **By id when there is one** (#150). The row was opened when
                // recording started and already owns this game's markers, so
                // it has to be the row that gets finished. `insert_recording`
                // upserts on `path` instead, and the path a recording started
                // with is only a prediction of the one it ends with: where the
                // two differ it would finish a *different* row and strand the
                // markers on an unfinished one nothing ever shows.
                //
                // The fallback is not dead code. A recording already in flight
                // when this version started has no id, and neither has one
                // whose start-insert failed; both finalize exactly as they did
                // before, which is the behaviour this is a strict improvement
                // on rather than a replacement for.
                let started_id = session.as_ref().and_then(|s| s.recording_id);
                let written = match started_id {
                    Some(id) => self.db.finish_recording(id, &row).map(|()| id),
                    None => self.db.insert_recording(&row),
                };

                let recording_id = match written {
                    Ok(id) => {
                        // Delete then insert, rather than append. Markers
                        // written during the game were resolved against
                        // whatever alignment was known at the time; the ones
                        // captured before game time first advanced used a 1:1
                        // fallback and are corrected here, against the
                        // alignment the whole game proved. Appending would
                        // leave both copies in the timeline.
                        //
                        // A delete that fails is worth reporting and worth
                        // continuing past: duplicated markers are a worse
                        // timeline than none, but both beat losing the row.
                        if let Err(e) = self.db.delete_markers(id) {
                            error!(
                                "state_machine",
                                "failed to clear the live markers for recording {id}: {e}"
                            );
                        }
                        let new_markers: Vec<db::NewMarker> =
                            markers.iter().map(marker_row).collect();
                        if let Err(e) = self.db.insert_markers(id, &new_markers) {
                            error!(
                                "state_machine",
                                "failed to insert markers for recording {id}: {e}"
                            );
                        }

                        // Same delete-then-insert as the markers, and for the
                        // same reason: the samples written during the game
                        // resolved against whatever alignment was known then.
                        if let Err(e) = self.db.delete_samples(id) {
                            error!(
                                "state_machine",
                                "failed to clear the live samples for recording {id}: {e}"
                            );
                        }
                        let new_samples: Vec<db::NewSample> =
                            samples.iter().map(sample_row).collect();
                        if let Err(e) = self.db.insert_samples(id, &new_samples) {
                            error!(
                                "state_machine",
                                "failed to insert samples for recording {id}: {e}"
                            );
                        }
                        Some(id)
                    }
                    Err(e) => {
                        error!("state_machine", "failed to insert recording row: {e}");
                        None
                    }
                };

                *self.last_finalized.lock().unwrap() = Some(FinalizedRecording {
                    recording_id,
                    path: path_str,
                    markers,
                });

                // Retention (DEVELOPMENT.md §6): enforced right
                // after every finalize, in addition to app-start
                // (lib.rs's `setup`) — this is what actually keeps disk
                // usage bounded during a long play session where the app
                // never restarts.
                match self.db.get_retention_policy() {
                    Ok(policy) => match crate::retention::enforce_now(&self.db, &policy) {
                        Ok(report) => {
                            if !report.deleted.is_empty() {
                                info!(
                                    "retention",
                                    "post-finalize enforcement: removed {} recording(s), freed {} bytes",
                                    report.deleted.len(),
                                    report.freed_bytes
                                );
                            }
                            // Published even when nothing was deleted: "the
                            // pass ran and took nothing" is the answer to
                            // "why is my disk still full", and the log line
                            // stays quiet for the same case so the two do
                            // not have to agree about noise.
                            self.publish(crate::contract::events::Event::RetentionRan {
                                deleted: report.deleted.clone(),
                                freed_bytes: report.freed_bytes,
                            });
                        }
                        Err(e) => error!("retention", "post-finalize enforcement failed: {e}"),
                    },
                    Err(e) => error!("retention", "failed to load policy: {e}"),
                }

                // After the row, its markers/samples, and any retention
                // deletions — one notification for the whole finalize.
                self.emit_library_changed(
                    crate::contract::events::LibraryChangeReason::Finalized,
                );
                // Separate from the library signal because it carries what was
                // written: the frontend refreshes off the first, a tray
                // notification describes the second.
                //
                // Note this still runs under the recorder lock, like the DB
                // writes above — recorder-then-db, so no lock-order inversion.
                // Whatever is behind this notifier must stay quick; it is not
                // the place to do work.
                if let Some(finalized) = self.last_finalized.lock().unwrap().clone() {
                    self.emit(SupervisorEvent::Finalized(finalized));
                }
                self.publish(crate::contract::events::Event::RecordingStopped {
                    recording_id,
                    outcome: crate::contract::events::StopOutcome::Clean,
                });

                // Last, and deliberately after retention: the LCU still
                // has no stats for this game — it is in `WaitingForStats`
                // — so `queue`, `role` and `patch` are filled in later,
                // off this path entirely. `crate::match_summary` owns the
                // waiting; this only hands over the identifiers.
                self.request_summary(recording_id, game, &live);
                // Same shape, same reason, and the size on the card is
                // whatever the trim leaves behind — so this follows the
                // library signal above rather than delaying it.
                if let Some(id) = recording_id {
                    self.request_trim(id);
                }
            }
            Err(e) => {
                error!("state_machine", "failed to stop recording: {e}");
                // The user is mid-game with the window closed; a failed
                // finalize is the one thing they cannot otherwise discover.
                self.emit(SupervisorEvent::RecordingFailed(format!(
                    "the recording could not be finished: {e}"
                )));
                // No id: the row is written from `stop`'s output, and there
                // was none. Whatever the recorder managed to write is still
                // on disk, which is what `Crashed` says.
                self.publish(crate::contract::events::Event::RecordingStopped {
                    recording_id: None,
                    outcome: crate::contract::events::StopOutcome::Crashed,
                });
            }
        }
    }

    /// Asks whoever is listening to fetch the LCU's post-game summary for
    /// the recording just written.
    ///
    /// Silent when there is nothing to do. No `recording_id` means the row
    /// write itself failed; no `game_id` means the gameflow read lost its
    /// race or there was no client to ask, which is simply what Practice
    /// Tool looks like. Neither is a problem worth a log line every game.
    fn request_summary(
        &self,
        recording_id: Option<i64>,
        game: lcu::GameIdentity,
        live: &LiveSummary,
    ) {
        let (Some(recording_id), Some(game_id)) = (recording_id, game.game_id) else {
            return;
        };
        // Stashed when the gameflow watch started, and deliberately not
        // cleared by `stop_gameflow_watch` — the client is still running
        // and this is exactly when it is needed.
        let Some(lockfile) = self.lockfile.lock().unwrap().clone() else {
            return;
        };

        if let Some(fetch) = self.summary_fetcher.lock().unwrap().as_ref() {
            fetch(crate::match_summary::SummaryRequest {
                recording_id,
                game_id,
                is_custom: game.is_custom,
                queue_id: game.queue_id,
                // Now, because this runs from the finalize: the game has just
                // ended, which is the whole reason a rank read here is about
                // this game and the same read tomorrow would not be.
                standing_before: self.rank_before.lock().unwrap().clone(),
                game_ended_at_ms: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0),
                lockfile,
                live: live.clone(),
            });
        }
    }
}

/// Dev-portal entry points into the otherwise-private async glue. These
/// exist because `machine.rs`'s pure transition function is well covered
/// by unit tests while *this* file — the part that actually spawns
/// watchers and drives the recorder — has never run against a real LCU
/// (DEVELOPMENT.md §3.4). Feeding it synthetic events from the dev portal
/// is the first exercise it gets.
#[cfg(feature = "devtools")]
impl Supervisor {
    /// Feeds one event through the real `dispatch`, executing whatever
    /// actions it returns for real: watchers are spawned, the recorder is
    /// started and stopped, DB rows are written. That is the point — this
    /// is not a dry run.
    pub fn dev_dispatch(self: &Arc<Self>, event: StateEvent) {
        self.dispatch(event);
    }

    /// Feeds one Live Client Data payload through the real marker/sample
    /// pipeline, exactly as the poller would.
    pub fn dev_on_snapshot(self: &Arc<Self>, snapshot: AllGameData) {
        self.on_snapshot(snapshot);
    }

    /// A read-only view of the in-flight recording session. Nothing else
    /// exposes this — `SupervisorStatus` only carries the *last finalized*
    /// recording, so markers and samples accumulating during a recording
    /// are otherwise invisible until it ends.
    pub fn dev_session_view(&self) -> Option<DevSessionView> {
        let guard = self.session.lock().unwrap();
        let session = guard.as_ref()?;
        Some(DevSessionView {
            marker_count: session.markers.len(),
            sample_count: session.samples.len(),
            alignment_offset_s: session.align.current_offset_s(),
            elapsed_s: session.record_started_at.elapsed().as_secs_f64(),
            started_at_millis: session.started_at_millis,
            recent_markers: session
                .resolved_markers()
                .into_iter()
                .rev()
                .take(20)
                .collect(),
            last_sample: session.resolved_samples().pop(),
        })
    }

    /// Emits `library-changed` on behalf of the dev commands, which mutate
    /// the DB directly rather than going through a finalize.
    pub fn dev_emit_library_changed(&self) {
        self.emit_library_changed(crate::contract::events::LibraryChangeReason::Edited);
    }
}

/// See `Supervisor::dev_session_view`. `alignment_offset_s` is `None` until
/// `gameTime` is first seen to advance — recording starts on the loading
/// screen, where the game clock is frozen at 0, so there is always a window
/// where the session exists but has no proven game-time-to-video-time
/// mapping yet. See `AlignmentTracker`.
#[cfg(feature = "devtools")]
#[derive(Debug, Clone, Serialize)]
pub struct DevSessionView {
    pub marker_count: usize,
    pub sample_count: usize,
    pub alignment_offset_s: Option<f64>,
    pub elapsed_s: f64,
    pub started_at_millis: i64,
    /// Newest first, capped — this is polled at 1 Hz by the portal.
    pub recent_markers: Vec<SessionMarker>,
    pub last_sample: Option<SessionSample>,
}

fn timestamp_millis() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    //! `start_recording`/`stop_recording` are the one piece of this
    //! module's async glue that genuinely can be tested without a live
    //! LCU/Live Client Data connection: they only touch the (already
    //! StubRecorder-backed) `Recorder` trait and the DB, both already
    //! exercised elsewhere. Everything else here (gameflow/lockfile/
    //! live-client watchers) is deliberately left untested per this
    //! file's header — no League client is installed on this machine.
    use super::*;
    // Aliased: `Event` alone would read as something this module owns, and
    // the tests below are specifically about what crosses the boundary.
    use crate::contract::events::{Event as ContractEvent, LibraryChangeReason};
    use crate::lcu::gameflow::GameflowPhase;
    use crate::lcu::lockfile::{LockfileInfo, LockfileState};
    use crate::db::Db;
    use crate::live_client::MarkerKind;
    use crate::recorder::stub::StubRecorder;
    use crate::recorder::RecorderError;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Counts `prepare`/`release` so the capture-backend lifecycle can be
    /// asserted on. The real bring-up only exists on Windows, so this is
    /// the only place the *policy* — warm with the client, cold at idle —
    /// is testable at all. Counters are shared with the test rather than
    /// read back out of the `Box<dyn Recorder>`, which can't be downcast.
    #[derive(Clone, Default)]
    struct Counts {
        prepared: Arc<AtomicUsize>,
        released: Arc<AtomicUsize>,
    }

    impl Counts {
        fn get(&self) -> (usize, usize) {
            (
                self.prepared.load(Ordering::Relaxed),
                self.released.load(Ordering::Relaxed),
            )
        }
    }

    struct CountingRecorder(Counts);

    impl Recorder for CountingRecorder {
        fn start(&mut self, _config: crate::recorder::RecordConfig) -> Result<(), RecorderError> {
            Ok(())
        }

        fn stop(&mut self) -> Result<crate::recorder::RecordingOutput, RecorderError> {
            Err(RecorderError::NotRecording)
        }

        fn is_recording(&self) -> bool {
            false
        }

        fn backend_name(&self) -> String {
            "counting".to_string()
        }

        fn prepare(&mut self) -> Result<(), RecorderError> {
            self.0.prepared.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }

        fn release(&mut self) {
            self.0.released.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn counting_supervisor() -> (Arc<Supervisor>, Counts) {
        let counts = Counts::default();
        let recorder: Arc<Mutex<Box<dyn Recorder>>> =
            Arc::new(Mutex::new(Box::new(CountingRecorder(counts.clone()))));
        let db = Arc::new(Db::open_temporary().unwrap());
        (
            Supervisor::new(recorder, std::env::temp_dir(), db),
            counts,
        )
    }

    // --- The event sink (WS2.3) -------------------------------------------

    /// A supervisor with a recording sink attached, so a test can assert on
    /// what crossed the contract boundary rather than on internal state.
    fn supervisor_with_sink() -> (Arc<Supervisor>, Arc<Mutex<Vec<ContractEvent>>>, PathBuf) {
        let (sup, dir) = test_supervisor();
        let seen: Arc<Mutex<Vec<ContractEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        sup.set_event_sink(Box::new(move |event| sink.lock().unwrap().push(event)));
        (sup, seen, dir)
    }

    /// Just the states, in order, from whatever else the run published.
    fn states(seen: &Arc<Mutex<Vec<ContractEvent>>>) -> Vec<GameState> {
        seen.lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                ContractEvent::StateChanged { state, .. } => Some(state.clone()),
                _ => None,
            })
            .collect()
    }

    fn present() -> StateEvent {
        StateEvent::LockfileChanged(LockfileState::Present(LockfileInfo {
            name: "LeagueClient".into(),
            pid: 2,
            port: 1,
            password: "x".into(),
            protocol: "https".into(),
        }))
    }

    /// **The exit criterion.** One `StateChanged` per transition, in order,
    /// across a whole game driven through the real dispatch path.
    ///
    /// `dispatch` runs the machine twice per call — the caller's event, then a
    /// blanket `FinalizeComplete` — so the end of the game produces two moves
    /// from one call, and both are here. That is the property worth pinning: an
    /// event per *transition*, not per `dispatch`.
    #[test]
    fn one_state_changed_per_transition_across_a_whole_game() {
        let (sup, seen, _dir) = supervisor_with_sink();

        sup.dispatch(present());
        sup.dispatch(StateEvent::GameflowPhase(GameflowPhase::InProgress));
        sup.dispatch(StateEvent::LiveClientUp);
        sup.dispatch(StateEvent::GameflowPhase(GameflowPhase::EndOfGame));

        assert_eq!(
            states(&seen),
            vec![
                GameState::ClientRunning,
                GameState::WaitingForGame,
                GameState::Recording,
                GameState::Finalizing,
                // FinalizeComplete, from the same `dispatch` as Finalizing:
                // the lockfile is still present, so it lands back here rather
                // than at Idle.
                GameState::ClientRunning,
            ]
        );
    }

    /// The other half of "one per transition": a handled event that does not
    /// move the state publishes nothing.
    ///
    /// Gameflow re-reports the phase it is already in, and `dispatch`'s blanket
    /// `FinalizeComplete` is a no-op in every state but `Finalizing`. Emitting
    /// on every `handle` would push a redraw at 1 Hz for a state nobody left.
    #[test]
    fn an_event_that_does_not_move_the_state_publishes_nothing() {
        let (sup, seen, _dir) = supervisor_with_sink();

        sup.dispatch(present());
        let after_first = states(&seen).len();
        assert_eq!(after_first, 1);

        // The same lockfile again, then a phase this state ignores.
        sup.dispatch(present());
        sup.dispatch(StateEvent::GameflowPhase(GameflowPhase::ChampSelect));
        sup.dispatch(StateEvent::LiveClientDown);

        assert_eq!(
            states(&seen).len(),
            after_first,
            "a handled-but-stationary event published a StateChanged"
        );
    }

    /// `since_ms` is the moment the state was entered, not the moment the
    /// event was rendered — so a client can show "recording for 4:12" without
    /// timing it, and without drifting when the window was asleep.
    #[test]
    fn since_ms_is_stamped_at_the_transition() {
        let (sup, seen, _dir) = supervisor_with_sink();
        let before = timestamp_millis();
        sup.dispatch(present());
        let after = timestamp_millis();

        let stamps: Vec<i64> = seen
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                ContractEvent::StateChanged { since_ms, .. } => Some(*since_ms),
                _ => None,
            })
            .collect();
        assert_eq!(stamps.len(), 1);
        assert!(
            stamps[0] >= before && stamps[0] <= after,
            "since_ms {} is outside [{before}, {after}]",
            stamps[0]
        );
    }

    /// The client going away is published as `phase: None`, which is a
    /// different statement from `GameflowPhase::None`.
    #[test]
    fn losing_the_client_publishes_an_absent_phase() {
        let (sup, seen, _dir) = supervisor_with_sink();
        sup.dispatch(present());
        sup.dispatch(StateEvent::LockfileChanged(LockfileState::Absent));

        let phases: Vec<(Option<GameflowPhase>, bool)> = seen
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                ContractEvent::LcuPhase {
                    phase,
                    client_present,
                } => Some((phase.clone(), *client_present)),
                _ => None,
            })
            .collect();
        assert_eq!(phases, vec![(None, false)]);
    }

    /// Installing no sink must leave every path exactly as it was. This is
    /// what every other test in this module relies on without saying so.
    #[test]
    fn a_supervisor_with_no_sink_publishes_nowhere_and_still_works() {
        let (sup, _dir) = test_supervisor();
        sup.dispatch(present());
        sup.dispatch(StateEvent::GameflowPhase(GameflowPhase::InProgress));
        assert_eq!(sup.status().state, GameState::WaitingForGame);
    }

    /// A finalize publishes the library change *and* names the reason, so a
    /// client can tell a one-row patch from a reconcile that moved many.
    #[test]
    fn a_library_change_names_its_reason() {
        let (sup, seen, _dir) = supervisor_with_sink();
        // The private emitter directly rather than `dev_emit_library_changed`,
        // which is behind the `devtools` feature — this behaviour is not.
        sup.emit_library_changed(LibraryChangeReason::Edited);

        let reasons: Vec<LibraryChangeReason> = seen
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                ContractEvent::LibraryChanged { reason } => Some(*reason),
                _ => None,
            })
            .collect();
        assert_eq!(reasons, vec![LibraryChangeReason::Edited]);
    }

    #[test]
    fn capture_backend_is_released_at_idle() {
        let (sup, counts) = counting_supervisor();
        sup.sync_capture_backend(&GameState::Idle);
        assert_eq!(counts.get(), (0, 1), "idle should release, never prepare");
    }

    #[test]
    fn capture_backend_is_prepared_once_the_client_is_running() {
        let (sup, counts) = counting_supervisor();
        for state in [
            GameState::ClientRunning,
            GameState::WaitingForGame,
            GameState::Recording,
            GameState::Finalizing,
        ] {
            let before = counts.get();
            sup.sync_capture_backend(&state);
            let after = counts.get();
            assert_eq!(after.0, before.0 + 1, "{state:?} should prepare");
            assert_eq!(after.1, before.1, "{state:?} should not release");
        }
    }

    #[test]
    fn a_client_restart_does_not_churn_the_capture_backend() {
        // A restart emits StopGameflowWatch + StartGameflowWatch while
        // staying in ClientRunning. Syncing off the state — not the actions
        // — means nothing is torn down and brought straight back up.
        let (sup, counts) = counting_supervisor();
        sup.sync_capture_backend(&GameState::ClientRunning);
        sup.sync_capture_backend(&GameState::ClientRunning);
        assert_eq!(counts.get().1, 0, "no release across a restart");
    }

    fn test_supervisor() -> (Arc<Supervisor>, PathBuf) {
        // `line!()` used to stand in for a per-test discriminator here, but
        // it expands at this call site, not the caller's — so it's a single
        // constant and every test shared one directory. With tests running
        // in parallel, whichever finished first would `remove_dir_all` the
        // directory another was still recording into, failing that test's
        // `Recorder::stop` about 1 run in 5. A real counter keeps them apart.
        static NEXT_DIR: AtomicUsize = AtomicUsize::new(0);

        let recorder: Arc<Mutex<Box<dyn Recorder>>> =
            Arc::new(Mutex::new(Box::new(StubRecorder::new())));
        let dir = std::env::temp_dir().join(format!(
            "ninja-recorder-supervisor-test-{}-{}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        let db = Arc::new(Db::open_temporary().unwrap());
        (Supervisor::new(recorder, dir.clone(), db), dir)
    }

    // --- Time alignment across a real poll sequence ----------------------
    //
    // `RecordingSession::ingest` takes `elapsed_s` as an argument precisely
    // so a whole game — loading screen, first blood, a pause — can be driven
    // here in microseconds. `on_snapshot` adds only the lock and the clock
    // read on top of this.

    /// One poll, built from the shared Live Client Data fixture with the
    /// game clock set and the event list narrowed to `event_ids` — so a test
    /// controls exactly which events the API has revealed by that poll.
    fn snapshot(game_time_s: f64, event_ids: &[i64]) -> AllGameData {
        let json = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../fixtures/live-client/sample-allgamedata.json"
        ));
        let mut data: AllGameData = serde_json::from_str(json).unwrap();
        data.game_data.game_time = game_time_s;
        data.events.events.retain(|e| event_ids.contains(&e.event_id));
        data
    }

    fn empty_session() -> RecordingSession {
        RecordingSession {
            // These tests drive `ingest` directly, with no database behind
            // them: `None` is the "the start-insert did not happen" case, and
            // exercising it here keeps the fallback path honest.
            recording_id: None,
            tracker: MarkerTracker::new(),
            markers: Vec::new(),
            samples: Vec::new(),
            align: AlignmentTracker::new(),
            live: LiveSummary::default(),
            scoreboard: None,
            polls: 0,
            first_game_time_s: None,
            last_game_time_s: None,
            ever_matched: false,
            game: lcu::GameIdentity::default(),
            last_live_written: None,
            record_started_at: Instant::now(),
            file_stem: "recording-0".to_string(),
            started_at_millis: 0,
        }
    }

    /// The regression this whole mechanism exists for. Recording starts on
    /// the first successful poll, which lands on the loading screen where
    /// `gameTime` is a frozen 0 — so the old "measure on the first poll"
    /// rule computed `0 - 0` and placed every marker at
    /// `video_time == game_time`, one entire loading screen early.
    #[test]
    fn markers_land_after_the_loading_screen_not_at_game_time() {
        let mut session = empty_session();

        // 11 seconds of loading screen at 1 Hz: the clock is stuck at 0
        // while the video records the whole thing.
        for i in 0..11 {
            session.ingest(&snapshot(0.0, &[]), i as f64);
        }
        // The clock starts running.
        session.ingest(&snapshot(1.0, &[]), 11.0);
        // First blood, 210.5s of game time later.
        session.ingest(&snapshot(210.5, &[3]), 221.0);

        let markers = session.resolved_markers();
        assert_eq!(markers.len(), 1, "the kill should be the only marker");
        assert_eq!(markers[0].marker.game_time_s, 210.5);
        assert_eq!(
            markers[0].video_time_s, 221.0,
            "the marker belongs at loading screen + game time, not at game time"
        );
    }

    /// A marker extracted before the clock was ever seen to move must still
    /// be kept — and, just as importantly, still be fed to the tracker, or
    /// its event ID never gets deduped and it reappears on every later poll.
    #[test]
    fn markers_seen_during_the_loading_screen_are_kept_and_deduped() {
        let mut session = empty_session();

        // The event is already in the payload while the clock is frozen.
        session.ingest(&snapshot(0.0, &[3]), 0.0);
        assert_eq!(session.markers.len(), 1, "kept, not dropped");
        assert!(
            session.markers[0].alignment.is_none(),
            "nothing was proven about the clock yet"
        );

        // Re-polled during the same loading screen: no duplicate.
        session.ingest(&snapshot(0.0, &[3]), 1.0);
        assert_eq!(session.markers.len(), 1, "the event ID was deduped");

        // The clock moves and proves an offset of 8s.
        session.ingest(&snapshot(1.0, &[3]), 9.0);
        assert_eq!(session.markers.len(), 1);

        let markers = session.resolved_markers();
        assert_eq!(
            markers[0].video_time_s, 218.5,
            "resolved against the first proven alignment (210.5 + 8)"
        );
    }

    /// The game ended during the loading screen, so no offset was ever
    /// proven. Markers fall back to a 1:1 mapping rather than being lost.
    #[test]
    fn markers_survive_a_game_whose_clock_never_moves() {
        let mut session = empty_session();
        for i in 0..5 {
            session.ingest(&snapshot(0.0, &[3]), i as f64);
        }

        let markers = session.resolved_markers();
        assert_eq!(markers.len(), 1);
        assert_eq!(markers[0].video_time_s, 210.5, "1:1 fallback");
    }

    /// Deferring the mapping is what makes this possible: a pause freezes
    /// the game clock while the video keeps rolling, so markers after it need
    /// a larger offset than markers before it. A single offset measured once
    /// cannot express that.
    #[test]
    fn markers_on_either_side_of_a_pause_use_different_offsets() {
        let mut session = empty_session();

        session.ingest(&snapshot(0.0, &[]), 5.0);
        // Clock live: offset 5s. A kill at 210.5 lands at 215.5.
        session.ingest(&snapshot(210.5, &[3]), 215.5);

        // 60s paused — the clock does not advance, so no re-derivation.
        for i in 0..60 {
            session.ingest(&snapshot(210.5, &[]), 216.5 + i as f64);
        }

        // Resumed. A dragon at 540 is now 60s further into the video than a
        // fixed offset would have put it.
        session.ingest(&snapshot(540.0, &[8]), 605.0);

        let markers = session.resolved_markers();
        assert_eq!(markers.len(), 2);
        assert_eq!(markers[0].video_time_s, 215.5, "before the pause");
        assert_eq!(
            markers[1].video_time_s, 605.0,
            "after the pause: 540 + 65, not 540 + 5"
        );
    }

    /// Samples are time-aligned the same way, and the advantage graph is
    /// drawn against `video_time_s`, so the same bug skewed the curve.
    #[test]
    fn samples_are_aligned_the_same_way_as_markers() {
        let mut session = empty_session();
        for i in 0..10 {
            session.ingest(&snapshot(0.0, &[]), i as f64);
        }
        session.ingest(&snapshot(1.0, &[]), 10.0);
        session.ingest(&snapshot(2.0, &[]), 11.0);

        let samples = session.resolved_samples();
        // One frozen-clock sample, then one per advancing poll.
        assert_eq!(samples.len(), 3);
        assert_eq!(samples[0].game_time_s, 0.0);
        assert_eq!(samples[0].video_time_s, 9.0, "fallback: first proven offset");
        assert_eq!(samples[2].game_time_s, 2.0);
        assert_eq!(samples[2].video_time_s, 11.0);
    }

    #[test]
    fn stop_recording_writes_recording_and_markers_to_db() {
        let (sup, dir) = test_supervisor();

        sup.start_recording();
        assert!(sup.session.lock().unwrap().is_some());

        // Simulate a marker collected mid-recording — normally done by
        // `on_snapshot`, which needs a live poll to drive it.
        {
            let mut guard = sup.session.lock().unwrap();
            let session = guard.as_mut().unwrap();
            session.markers.push(PendingMarker {
                marker: Marker {
                    kind: MarkerKind::Kill,
                    game_time_s: 12.5,
                    payload: serde_json::json!({ "victim": "EnemyA" }),
                },
                alignment: Some(TimeAlignment::new(12.5, 15.0)),
            });
        }

        // Likewise a sample — normally pushed by `on_snapshot`.
        {
            let mut guard = sup.session.lock().unwrap();
            let session = guard.as_mut().unwrap();
            session.samples.push(PendingSample {
                game_time_s: 12.0,
                diff: Some(live_client::TeamDiff {
                    our_team: "CHAOS".into(),

                    kill_diff: -2,
                    cs_diff: 15,
                }),
                our_gold: 450.0,
                our_level: 11,
                alignment: Some(TimeAlignment::new(12.0, 14.5)),
            });
        }

        sup.stop_recording();

        let finalized = sup
            .status()
            .last_finalized
            .expect("stop_recording should set last_finalized");
        assert!(finalized.recording_id.is_some(), "DB write should have succeeded");
        assert_eq!(finalized.markers.len(), 1);

        // Samples must land alongside the markers, signs intact — a
        // recording that finalizes without them renders a blank graph.
        let samples = sup.db.get_samples(finalized.recording_id.unwrap()).unwrap();
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].our_team, Some("CHAOS".to_string()));
        // Gold is not a live number any more; the finalize writes none.
        assert_eq!(samples[0].gold_diff, None);
        assert_eq!(samples[0].kill_diff, Some(-2));

        let rows = sup.db.list_recordings().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, finalized.recording_id.unwrap());

        // The library's Length column and its Recorded total both read this;
        // a NULL here is what made every card say "unknown".
        let duration = rows[0]
            .duration_s
            .expect("finalize should record how long the capture ran");
        assert!(
            duration >= 0.0,
            "duration should come from the session clock, got {duration}"
        );

        assert!(
            sup.session.lock().unwrap().is_none(),
            "session should be cleared after stop"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A whole fixture file, unmodified — as against `snapshot` above,
    /// which narrows the shared fixture down to one poll of a game.
    fn fixture_snapshot(name: &str) -> AllGameData {
        let json = match name {
            "mid-game" => include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../fixtures/live-client/sample-allgamedata.json"
            )),
            "won" => include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../fixtures/live-client/game-end-win.json"
            )),
            // Trimmed to have no `allPlayers` at all, so `find_us` cannot
            // place us — the state a NULL champion comes from.
            "unmatched" => include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../fixtures/live-client/summoner-name-mismatch.json"
            )),
            other => panic!("no such fixture: {other}"),
        };
        serde_json::from_str(json).unwrap()
    }

    /// The end-to-end shape of the live metadata path: polls arrive, the
    /// session accumulates, the finalize writes it. Everything the library
    /// card shows without the LCU comes through here.
    #[test]
    fn a_finalized_recording_carries_what_the_live_client_reported() {
        let (sup, dir) = test_supervisor();
        sup.start_recording();

        // A mid-game poll, then the one carrying GameEnd — which in a real
        // game is routinely the last poll that ever succeeds.
        sup.on_snapshot(fixture_snapshot("mid-game"));
        sup.on_snapshot(fixture_snapshot("won"));

        sup.stop_recording();

        let rows = sup.db.list_recordings().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].champion.as_deref(), Some("Ahri"));
        assert_eq!(rows[0].win, Some(true));
        assert_eq!(rows[0].kda_k, Some(3));
        assert_eq!(rows[0].kda_d, Some(1));
        assert_eq!(rows[0].kda_a, Some(2));
        assert_eq!(rows[0].game_mode.as_deref(), Some("CLASSIC"));

        std::fs::remove_dir_all(&dir).ok();
    }

    // --- A killed daemon keeps its markers (#150) -------------------------

    /// **The bug, reproduced.** Markers used to live in `session.markers`
    /// for the whole game and reach SQLite once, at finalize, so a daemon
    /// killed mid-game took every one of them with it while leaving a
    /// perfectly playable file behind.
    ///
    /// Not calling `stop_recording` is the kill: that is precisely what a
    /// process dying does, and what the state machine's finalize is the only
    /// caller of.
    #[test]
    fn a_killed_daemon_leaves_its_markers_in_the_database() {
        let (sup, dir) = test_supervisor();
        sup.start_recording();
        sup.on_snapshot(snapshot(210.5, &[3]));

        // The daemon dies here. No finalize, ever.

        let open = sup.db.unfinished_recordings().unwrap();
        assert_eq!(open.len(), 1, "the recording opened a row when it started");

        let markers = sup.db.get_markers(open[0].id).unwrap();
        assert_eq!(markers.len(), 1, "written as it arrived, not held for a finalize");
        assert_eq!(markers[0].game_time_s, 210.5);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// The other half of the same decision: the row exists, and it is still
    /// not a library entry. A card with no duration, no champion and a
    /// growing file behind it is not something to put in front of someone,
    /// and one left by a crash would sit there forever looking broken.
    #[test]
    fn the_row_a_recording_opens_is_not_in_the_library_yet() {
        let (sup, dir) = test_supervisor();
        sup.start_recording();
        sup.on_snapshot(snapshot(210.5, &[3]));

        assert!(
            sup.db.list_recordings().unwrap().is_empty(),
            "an unfinished recording is not a library entry"
        );
        assert_eq!(sup.db.unfinished_recordings().unwrap().len(), 1);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A finalize must complete the row the recording opened, not add a
    /// second one beside it. Getting this wrong strands the markers written
    /// during the game on a row nothing ever shows.
    #[test]
    fn a_finalize_completes_the_row_the_recording_opened() {
        let (sup, dir) = test_supervisor();
        sup.start_recording();
        let opened = sup.db.unfinished_recordings().unwrap()[0].id;

        sup.on_snapshot(snapshot(210.5, &[3]));
        sup.stop_recording();

        let rows = sup.db.list_recordings().unwrap();
        assert_eq!(rows.len(), 1, "one row, not one per phase");
        assert_eq!(rows[0].id, opened, "the same row, finished");
        assert!(sup.db.unfinished_recordings().unwrap().is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }

    /// The finalize deletes and re-inserts rather than appending. Appending
    /// would leave every marker in the timeline twice: once from the poll
    /// that saw it, once from the finalize that resolved it against the
    /// alignment the whole game proved.
    #[test]
    fn a_finalize_replaces_the_live_markers_rather_than_appending() {
        let (sup, dir) = test_supervisor();
        sup.start_recording();

        // Two polls carrying the same event, then a third carrying a
        // second. The tracker dedupes by event id, so this is two markers
        // written live across three polls.
        sup.on_snapshot(snapshot(210.5, &[3]));
        sup.on_snapshot(snapshot(211.5, &[3]));
        sup.on_snapshot(snapshot(540.0, &[3, 8]));

        let id = sup.db.unfinished_recordings().unwrap()[0].id;
        let live = sup.db.get_markers(id).unwrap().len();
        assert_eq!(live, 2, "deduped by event id while the game ran");

        sup.stop_recording();

        assert_eq!(
            sup.db.get_markers(id).unwrap().len(),
            live,
            "the finalize rewrites the markers, it does not add to them"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// The correction the delete-and-reinsert exists for. A marker seen
    /// before game time ever advanced is resolved against a 1:1 fallback
    /// when it is written live, and against the alignment the game proved
    /// once there is one. The durable answer is the second.
    #[test]
    fn the_finalize_corrects_a_marker_written_before_the_clock_moved() {
        let (sup, dir) = test_supervisor();
        sup.start_recording();

        // The loading screen: game time pinned at 0 while capture runs on.
        // The event's own clock reads 210.5, and with no alignment proven
        // yet it resolves 1:1 against that.
        sup.on_snapshot(snapshot(0.0, &[3]));
        let id = sup.db.unfinished_recordings().unwrap()[0].id;
        let live = sup.db.get_markers(id).unwrap()[0].video_time_s;
        assert_eq!(live, 210.5, "the 1:1 fallback, which is all there is to go on");

        // Now the clock jumps to 30s while barely any capture time has
        // passed, which proves an offset of about -30.
        sup.on_snapshot(snapshot(30.0, &[3]));
        sup.stop_recording();

        let settled = sup.db.get_markers(id).unwrap();
        assert_eq!(settled.len(), 1);
        assert_ne!(
            settled[0].video_time_s, live,
            "the live value was provisional and the finalize was supposed to replace it"
        );
        // 210.5 game seconds, less the ~30s the alignment proved the video
        // runs behind by. Within a second because `elapsed_s` is a real clock.
        assert!(
            (settled[0].video_time_s - 180.5).abs() < 1.0,
            "resolved against the proven alignment, not the fallback: {}",
            settled[0].video_time_s
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    // --- A killed daemon keeps its curve too ------------------------------

    /// The other half of #150, which covered only the markers. Samples were
    /// held in `session.samples` for the whole game and written once, at
    /// finalize, so a daemon killed mid-game left a recovered recording with
    /// its markers intact and an empty advantage graph behind them.
    #[test]
    fn a_killed_daemon_leaves_its_samples_in_the_database() {
        let (sup, dir) = test_supervisor();
        sup.start_recording();
        sup.on_snapshot(snapshot(210.5, &[3]));

        // The daemon dies here. No finalize, ever.

        let open = sup.db.unfinished_recordings().unwrap();
        let samples = sup.db.get_samples(open[0].id).unwrap();
        assert_eq!(samples.len(), 1, "written as the poll produced it");
        assert_eq!(samples[0].game_time_s, 210.5);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// One sample per poll that moved the clock, and none from one that did
    /// not. `ingest` skips a repeated timestamp so the graph has no vertical
    /// artefact through it, and the live write has to skip exactly the same
    /// polls or it would reintroduce one.
    #[test]
    fn a_poll_that_does_not_move_the_clock_writes_no_sample() {
        let (sup, dir) = test_supervisor();
        sup.start_recording();

        let id = sup.db.unfinished_recordings().unwrap()[0].id;

        sup.on_snapshot(snapshot(210.5, &[3]));
        assert_eq!(sup.db.get_samples(id).unwrap().len(), 1);

        // The loading-screen and pause case: the poller re-fetches the same
        // payload, game time unchanged.
        sup.on_snapshot(snapshot(210.5, &[3]));
        assert_eq!(
            sup.db.get_samples(id).unwrap().len(),
            1,
            "a repeated timestamp is not a second point on the curve"
        );

        sup.on_snapshot(snapshot(211.5, &[3]));
        assert_eq!(sup.db.get_samples(id).unwrap().len(), 2);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// The finalize deletes and re-inserts the samples rather than appending,
    /// exactly as it does the markers. Appending would draw every point on
    /// the curve twice.
    #[test]
    fn a_finalize_replaces_the_live_samples_rather_than_appending() {
        let (sup, dir) = test_supervisor();
        sup.start_recording();

        sup.on_snapshot(snapshot(210.5, &[3]));
        sup.on_snapshot(snapshot(211.5, &[3]));
        sup.on_snapshot(snapshot(212.5, &[3]));

        let id = sup.db.unfinished_recordings().unwrap()[0].id;
        let live = sup.db.get_samples(id).unwrap().len();
        assert_eq!(live, 3, "one per poll that moved the clock");

        sup.stop_recording();

        assert_eq!(
            sup.db.get_samples(id).unwrap().len(),
            live,
            "the finalize rewrites the samples, it does not add to them"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Why the delete-and-reinsert is there rather than an insert guard. A
    /// sample taken before the clock was ever seen to advance is resolved
    /// against a 1:1 fallback when it is written live, and against the
    /// alignment the game proved once there is one. The durable answer is the
    /// second, and it is the one the curve's x-axis is drawn from.
    #[test]
    fn the_finalize_corrects_a_sample_written_before_the_clock_moved() {
        let (sup, dir) = test_supervisor();
        sup.start_recording();

        // The first poll of a game already in progress. Nothing has proven an
        // offset yet, so the sample resolves 1:1 against its own game time.
        sup.on_snapshot(snapshot(210.5, &[3]));
        let id = sup.db.unfinished_recordings().unwrap()[0].id;
        let live = sup.db.get_samples(id).unwrap()[0].video_time_s;
        assert_eq!(live, 210.5, "the 1:1 fallback, which is all there is to go on");

        // The clock advances while capture has barely run, which proves that
        // game second 211.5 is the start of this video rather than 211 seconds
        // into it.
        sup.on_snapshot(snapshot(211.5, &[3]));
        sup.stop_recording();

        let settled = sup.db.get_samples(id).unwrap();
        assert_eq!(settled.len(), 2);
        assert_ne!(
            settled[0].video_time_s, live,
            "the live value was provisional and the finalize was supposed to replace it"
        );
        // A second before capture began, and `video_time_s` clamps at the
        // start of the file.
        assert_eq!(settled[0].video_time_s, 0.0);

        std::fs::remove_dir_all(&dir).ok();
    }

    // --- A killed daemon keeps its card too -------------------------------

    /// The third thing a crash used to take, after the markers (#150) and the
    /// curve (#185). Champion, KDA, mode and outcome lived in `session.live`
    /// for the whole game and reached SQLite once, at finalize, so a daemon
    /// killed mid-game left a recovered recording with a card that had no
    /// title on it. The polls had known since the first one.
    #[test]
    fn a_killed_daemon_leaves_its_card_filled_in() {
        let (sup, dir) = test_supervisor();
        sup.start_recording();
        sup.on_snapshot(fixture_snapshot("mid-game"));

        // The daemon dies here. No finalize, ever.

        let open = sup.db.unfinished_recordings().unwrap();
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].champion.as_deref(), Some("Ahri"));
        assert_eq!(open[0].game_mode.as_deref(), Some("CLASSIC"));
        assert_eq!(open[0].kda_k, Some(3));
        assert_eq!(open[0].kda_d, Some(1));
        assert_eq!(open[0].kda_a, Some(2));
        assert_eq!(open[0].win, None, "the game had not ended, so it is not a loss");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// The outcome is the field the live write exists for as much as the
    /// champion is. `GameEnd` arrives on one poll and the game process can
    /// exit before the next, so a recording killed seconds later still knows
    /// it won.
    #[test]
    fn an_outcome_seen_before_the_kill_is_on_the_row() {
        let (sup, dir) = test_supervisor();
        sup.start_recording();
        sup.on_snapshot(fixture_snapshot("mid-game"));
        sup.on_snapshot(fixture_snapshot("won"));

        let open = sup.db.unfinished_recordings().unwrap();
        assert_eq!(open[0].win, Some(true));

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A poll that establishes nothing new is not a write. Most polls in a
    /// game are that poll: a KDA moves on a kill, not on a tick, and the
    /// loading screen and the end-of-game screen produce nothing but repeats.
    ///
    /// Tested through the comparison rather than through a write counter,
    /// because the comparison is the decision. A row that changed shape but
    /// not content would still compare equal and still be skipped, which is
    /// the property worth pinning.
    #[test]
    fn a_poll_that_establishes_nothing_new_is_not_a_write() {
        let mut session = empty_session();

        session.ingest(&fixture_snapshot("mid-game"), 1.0);
        let first = session.live_row();
        assert_eq!(first.champion.as_deref(), Some("Ahri"));

        session.ingest(&fixture_snapshot("mid-game"), 2.0);
        assert_eq!(session.live_row(), first, "the same payload, so nothing to write");

        session.ingest(&fixture_snapshot("won"), 3.0);
        assert_ne!(session.live_row(), first, "the outcome arrived");
        assert_eq!(session.live_row().win, Some(true));
    }

    /// The finalize still owns the row. The live write is not a second
    /// opinion about a finished recording: everything it set is rewritten,
    /// and anything only the finalize knows lands beside it.
    #[test]
    fn the_finalize_still_writes_the_whole_row() {
        let (sup, dir) = test_supervisor();
        sup.start_recording();
        sup.on_snapshot(fixture_snapshot("mid-game"));
        sup.on_snapshot(fixture_snapshot("won"));
        sup.stop_recording();

        let rows = sup.db.list_recordings().unwrap();
        assert_eq!(rows.len(), 1, "one row, still finished by id");
        assert_eq!(rows[0].champion.as_deref(), Some("Ahri"));
        assert_eq!(rows[0].win, Some(true));
        assert!(
            rows[0].duration_s.is_some(),
            "the session clock is the finalize's to write, and it did"
        );
        assert!(sup.db.unfinished_recordings().unwrap().is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }

    /// The fourth thing a crash used to take, after the markers (#150), the
    /// curve (#185) and the card (#190). The identity is read once per game
    /// from the gameflow session and was held on the supervisor until the
    /// finalize, so a killed daemon left a row that could only ever be
    /// matched back to its game on the clock, and only while the client still
    /// remembered the game at all.
    #[test]
    fn a_killed_daemon_leaves_the_game_identity_on_the_row() {
        let (sup, dir) = test_supervisor();
        *sup.pending_game.lock().unwrap() = lcu::GameIdentity {
            game_id: Some(5147823901),
            queue_id: Some(420),
            is_custom: false,
        };

        sup.start_recording();
        sup.on_snapshot(fixture_snapshot("mid-game"));

        // The daemon dies here. No finalize, ever.

        let open = sup.db.unfinished_recordings().unwrap();
        assert_eq!(open[0].game_id, Some(5147823901));
        assert_eq!(open[0].queue, Some(420));

        std::fs::remove_dir_all(&dir).ok();
    }

    /// The identity is absorbed, not re-read. The gameflow watch is torn down
    /// with the transition out of `InProgress`, so a read taken after that
    /// comes back empty, and an empty read is the client not answering rather
    /// than the game having had no id.
    #[test]
    fn a_client_that_goes_away_cannot_take_the_identity_back() {
        let (sup, dir) = test_supervisor();
        *sup.pending_game.lock().unwrap() = lcu::GameIdentity {
            game_id: Some(5147823901),
            queue_id: Some(420),
            is_custom: false,
        };

        sup.start_recording();
        sup.on_snapshot(fixture_snapshot("mid-game"));

        // The client drops out: the next read has nothing in it.
        *sup.pending_game.lock().unwrap() = lcu::GameIdentity::default();
        sup.on_snapshot(fixture_snapshot("won"));

        let id = sup.db.unfinished_recordings().unwrap()[0].id;
        assert_eq!(
            sup.db.get_recording(id).unwrap().unwrap().game_id,
            Some(5147823901),
            "the row keeps what it read while the game was running"
        );

        sup.stop_recording();
        let rows = sup.db.list_recordings().unwrap();
        assert_eq!(rows[0].game_id, Some(5147823901), "and so does the finalize");
        assert_eq!(rows[0].queue, Some(420));

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A recording with no successful poll never absorbs anything, so the
    /// finalize has to fall back to the supervisor's own copy. This is the
    /// path `the_identified_game_lands_on_the_finalized_row` already covers
    /// from the other direction; here it is with the session in play.
    #[test]
    fn a_game_with_no_polls_still_gets_its_identity_at_finalize() {
        let (sup, dir) = test_supervisor();
        *sup.pending_game.lock().unwrap() = lcu::GameIdentity {
            game_id: Some(7),
            queue_id: Some(0),
            is_custom: true,
        };

        sup.start_recording();
        sup.stop_recording();

        let rows = sup.db.list_recordings().unwrap();
        assert_eq!(rows[0].game_id, Some(7));
        assert_eq!(rows[0].queue, Some(0), "a custom game's queue id is zero, not absent");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// The identity is resolved when gameflow reaches `InProgress`, long
    /// before this recording's session exists, so it lives on the
    /// supervisor and is read at finalize. Set here directly because the
    /// fetch itself needs a live client.
    #[test]
    fn the_identified_game_lands_on_the_finalized_row() {
        let (sup, dir) = test_supervisor();
        *sup.pending_game.lock().unwrap() = lcu::GameIdentity {
            game_id: Some(5147823901),
            queue_id: Some(420),
            is_custom: false,
        };

        sup.start_recording();
        sup.stop_recording();

        let rows = sup.db.list_recordings().unwrap();
        assert_eq!(rows[0].game_id, Some(5147823901));
        assert_eq!(rows[0].queue, Some(420));

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Queue 0 is a custom game, and the library labels it "Custom". If
    /// this were treated as absent the whole row would lose its queue.
    #[test]
    fn a_custom_games_queue_id_of_zero_survives_to_the_row() {
        let (sup, dir) = test_supervisor();
        *sup.pending_game.lock().unwrap() = lcu::GameIdentity {
            game_id: Some(7),
            queue_id: Some(0),
            is_custom: true,
        };

        sup.start_recording();
        sup.stop_recording();

        assert_eq!(sup.db.list_recordings().unwrap()[0].queue, Some(0));

        std::fs::remove_dir_all(&dir).ok();
    }

    // --- recording diagnostics (#71) --------------------------------------

    fn diagnostics_of(sup: &Supervisor) -> RecordingDiagnostics {
        let rows = sup.db.list_recordings().unwrap();
        let json = rows[0]
            .diagnostics_json
            .as_ref()
            .expect("a finalize should always record what it observed");
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn a_finalize_records_what_it_observed_not_just_what_it_wrote() {
        let (sup, dir) = test_supervisor();
        *sup.pending_game.lock().unwrap() = lcu::GameIdentity {
            game_id: Some(5147823901),
            queue_id: Some(420),
            is_custom: false,
        };

        sup.start_recording();
        sup.on_snapshot(fixture_snapshot("mid-game"));
        sup.on_snapshot(fixture_snapshot("won"));
        sup.stop_recording();

        let d = diagnostics_of(&sup);
        assert_eq!(d.game_id, Some(5147823901));
        assert_eq!(d.queue_id, Some(420));
        assert!(!d.is_custom);
        assert_eq!(d.polls, 2, "both polls should be counted");
        assert!(d.ever_matched);
        assert_eq!(d.backend, "stub");
        assert!(d.first_game_time_s.is_some());
        assert!(d.last_game_time_s.is_some());

        std::fs::remove_dir_all(&dir).ok();
    }

    /// The single most useful thing in the record. A game where we were
    /// never found in `allPlayers` has a NULL champion, a NULL KDA and an
    /// empty advantage curve, and nothing else afterwards says why — the
    /// payload is gone the moment the game ends.
    #[test]
    fn a_game_we_were_never_placed_in_says_so() {
        let (sup, dir) = test_supervisor();
        sup.start_recording();
        sup.on_snapshot(fixture_snapshot("unmatched"));
        sup.stop_recording();

        let d = diagnostics_of(&sup);
        assert!(!d.ever_matched);
        assert_eq!(d.polls, 1);
        // And the row it explains.
        assert_eq!(sup.db.list_recordings().unwrap()[0].champion, None);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A recording whose poller never came up at all still gets a record,
    /// and the record is what says the poller never came up.
    #[test]
    fn a_recording_with_no_polls_still_records_that_it_had_none() {
        let (sup, dir) = test_supervisor();
        sup.start_recording();
        sup.stop_recording();

        let d = diagnostics_of(&sup);
        assert_eq!(d.polls, 0);
        assert_eq!(d.first_game_time_s, None);
        assert_eq!(d.alignment_offset_s, None, "no clock, so no proven offset");
        assert!(!d.ever_matched);
        assert_eq!(d.game_id, None);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// `reconcile` upserts on `path` with an all-default row, so a rescan
    /// landing after a finalize must not erase the record — the same rule
    /// `audio_tracks_json` and `game_mode` already have.
    #[test]
    fn a_rescan_upsert_cannot_erase_the_diagnostics() {
        let (sup, dir) = test_supervisor();
        sup.start_recording();
        sup.on_snapshot(fixture_snapshot("mid-game"));
        sup.stop_recording();

        let path = sup.db.list_recordings().unwrap()[0].path.clone();
        sup.db
            .insert_recording(&db::NewRecording {
                path,
                started_at: 1,
                ..Default::default()
            })
            .unwrap();

        assert_eq!(diagnostics_of(&sup).polls, 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    // --- the deferred summary request ------------------------------------

    fn a_lockfile() -> lcu::LockfileInfo {
        lcu::LockfileInfo {
            name: "LeagueClient".into(),
            pid: 1234,
            port: 2999,
            password: "hunter2".into(),
            protocol: "https".into(),
        }
    }

    /// Collects what `stop_recording` hands over, standing in for the
    /// closure `lib.rs` installs — which spawns a task, which is exactly
    /// what must not happen in a test binary.
    fn recording_fetcher(sup: &Supervisor) -> Arc<Mutex<Vec<crate::match_summary::SummaryRequest>>> {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        sup.set_summary_fetcher(Box::new(move |request| {
            sink.lock().unwrap().push(request);
        }));
        seen
    }

    /// The finalize's whole contribution to the LCU patch: hand over the
    /// identifiers and return. Everything else — the HTTP client, the
    /// retry schedule, the UPDATE — lives behind this seam in
    /// `crate::match_summary`.
    #[test]
    fn a_finalize_asks_for_the_summary_of_the_game_it_identified() {
        let (sup, dir) = test_supervisor();
        let seen = recording_fetcher(&sup);

        sup.start_recording();
        sup.on_snapshot(fixture_snapshot("won"));
        // Set after the poll, not before: this test drives the supervisor
        // directly, so the machine is still `Idle`, and a dispatch from
        // `Idle` clears the lockfile stash on purpose (`dispatch_one` —
        // idle means the client is gone). In a real game the machine is in
        // `Recording` and both of these were established long before.
        *sup.lockfile.lock().unwrap() = Some(a_lockfile());
        *sup.pending_game.lock().unwrap() = lcu::GameIdentity {
            game_id: Some(5147823901),
            queue_id: Some(420),
            is_custom: false,
        };

        sup.stop_recording();

        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].game_id, 5147823901);
        assert!(!seen[0].is_custom);
        assert_eq!(
            seen[0].recording_id,
            sup.db.list_recordings().unwrap()[0].id,
            "the request must name the row that was just written"
        );
        // Carried so the patch can report a disagreement rather than
        // silently overwriting one source with the other.
        assert_eq!(seen[0].live.win, Some(true));

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Practice Tool, or a game where the gameflow read lost its race.
    /// There is nothing to fetch a summary *by*, and asking anyway would
    /// mean a minute of retries against a game id we do not have.
    #[test]
    fn no_game_id_means_no_summary_is_asked_for() {
        let (sup, dir) = test_supervisor();
        let seen = recording_fetcher(&sup);
        *sup.lockfile.lock().unwrap() = Some(a_lockfile());

        sup.start_recording();
        sup.stop_recording();

        assert!(seen.lock().unwrap().is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The client exited between the game and the finalize. Nothing to
    /// ask, and `discover`ing a fresh one would risk answering about a
    /// different client session than the one that played the game.
    #[test]
    fn no_lockfile_means_no_summary_is_asked_for() {
        let (sup, dir) = test_supervisor();
        let seen = recording_fetcher(&sup);
        *sup.pending_game.lock().unwrap() = lcu::GameIdentity {
            game_id: Some(5147823901),
            queue_id: Some(420),
            is_custom: false,
        };

        sup.start_recording();
        sup.stop_recording();

        assert!(seen.lock().unwrap().is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The acceptance criterion for the whole seam: with nothing
    /// installed, a finalize is exactly what it was before this existed.
    /// Every other test in this module relies on it.
    #[test]
    fn a_finalize_with_no_fetcher_installed_still_writes_its_row() {
        let (sup, dir) = test_supervisor();
        *sup.lockfile.lock().unwrap() = Some(a_lockfile());
        *sup.pending_game.lock().unwrap() = lcu::GameIdentity {
            game_id: Some(5147823901),
            queue_id: Some(420),
            is_custom: false,
        };

        sup.start_recording();
        sup.stop_recording();

        assert_eq!(sup.db.list_recordings().unwrap().len(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A game the poller never reached — it crashed on the loading screen,
    /// or port 2999 never came up. The footage still has to land in the
    /// library; it just lands without metadata, as it always has.
    #[test]
    fn a_recording_with_no_live_data_still_writes_its_row() {
        let (sup, dir) = test_supervisor();
        sup.start_recording();
        sup.stop_recording();

        let rows = sup.db.list_recordings().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].champion, None);
        assert_eq!(rows[0].win, None);
        assert_eq!(rows[0].game_mode, None);
        assert_eq!(rows[0].game_id, None, "no LCU, so no game id");
        assert_eq!(rows[0].queue, None);
        assert!(
            rows[0].duration_s.is_some(),
            "the session clock does not depend on the live client"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn finalize_for_shutdown_is_a_no_op_with_nothing_recording() {
        let (sup, _dir) = test_supervisor();
        assert!(
            !sup.finalize_for_shutdown(),
            "nothing was recording, so there was nothing to finalize"
        );
        assert!(sup.status().last_finalized.is_none());
    }

    #[test]
    fn finalize_for_shutdown_writes_the_recording_so_quitting_cannot_lose_it() {
        let (sup, dir) = test_supervisor();
        sup.start_recording();
        assert!(sup.session.lock().unwrap().is_some());

        assert!(sup.finalize_for_shutdown(), "a recording was in flight");

        let finalized = sup
            .status()
            .last_finalized
            .expect("quitting mid-recording must still write the row");
        assert!(finalized.recording_id.is_some(), "DB write should have succeeded");
        assert!(sup.session.lock().unwrap().is_none(), "session should be cleared");

        // Same finalize path, so the duration must survive the tray's Quit too.
        let rows = sup.db.list_recordings().unwrap();
        assert!(rows[0].duration_s.is_some());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn stop_recording_without_start_writes_nothing() {
        let (sup, dir) = test_supervisor();

        // StubRecorder::stop() without a prior start() errors — nothing
        // should reach the DB.
        sup.stop_recording();

        assert!(sup.status().last_finalized.is_none());
        assert!(sup.db.list_recordings().unwrap().is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }
}
